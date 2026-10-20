//! Exact observation of the dynamically linked glibc malloc family.
use super::capture::{CaptureOptions, done, spawn};
use super::{HeapFrame, HeapMapping, HeapProfile, NativeHeapTracker};
use crate::{LoadedImage, NativeSymbolizer, PprofProfile, Symbolizer};
use anyhow::{Context, Result, ensure};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

#[cfg(target_env = "gnu")]
pub fn recorder_library() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/libpprofessor_heap.so"))
}
#[cfg(not(target_env = "gnu"))]
pub fn recorder_library() -> &'static [u8] {
    &[]
}

pub(super) fn run(
    binary: &OsStr,
    args: &[OsString],
    options: &CaptureOptions,
    stop: &AtomicBool,
    workspace: &Path,
    checkpoint: &mut impl FnMut(u32, &PprofProfile),
) -> Result<PprofProfile> {
    ensure!(
        !recorder_library().is_empty(),
        "native heap capture currently requires Linux glibc"
    );
    let library = workspace.join("libpprofessor_heap.so");
    std::fs::write(&library, recorder_library())?;
    let log = workspace.join("allocations");
    let mut command = Command::new(binary);
    command
        .args(args)
        .env("LD_PRELOAD", &library)
        .env("PPROFESSOR_HEAP_LOG", &log);
    ensure!(
        std::env::var_os("LD_PRELOAD").is_none(),
        "heap capture cannot safely compose with an existing LD_PRELOAD allocator"
    );
    let mut child = spawn(command)?;
    let pid = child.0.id();
    let spawned = Instant::now();
    let mut start = spawned;
    let mut next = Instant::now() + options.interval;
    let mut reader = None;
    loop {
        let exited = child.0.try_wait()?.is_some();
        if reader.is_none() && log.metadata().is_ok_and(|m| m.len() >= 32) {
            reader = Some(LogReader::open(&log, pid)?);
            start = Instant::now();
            next = start + options.interval;
        }
        if reader.is_none()
            && (spawned.elapsed() > Duration::from_secs(10)
                || stop.load(std::sync::atomic::Ordering::Relaxed))
        {
            anyhow::bail!(
                "native heap recorder did not initialize; capture requires an instrumentable glibc target"
            );
        }
        if exited || (reader.is_some() && done(start, options, stop)) || Instant::now() >= next {
            if let Some(reader) = reader.as_mut() {
                let profile = reader.snapshot(exited)?.to_pprof()?;
                checkpoint(pid, &profile);
                if exited || done(start, options, stop) {
                    return Ok(profile);
                }
            } else if exited || done(start, options, stop) {
                anyhow::bail!(
                    "heap recorder did not initialize: target must be a dynamically linked glibc executable of the profiler's architecture; static, secure-execution and custom-allocator targets are not supported"
                );
            }
            next = Instant::now() + options.interval;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Read an existing recorder started with PPROFESSOR_HEAP_LOG and LD_PRELOAD.
pub fn attach(
    pid: u32,
    log: &Path,
    options: &CaptureOptions,
    stop: &AtomicBool,
    checkpoint: &mut impl FnMut(u32, &PprofProfile),
) -> Result<PprofProfile> {
    let mut reader = LogReader::open(log, pid)?;
    let start = Instant::now();
    loop {
        let alive = crate::process_exists(pid);
        let profile = reader.snapshot(!alive)?.to_pprof()?;
        checkpoint(pid, &profile);
        if !alive || done(start, options, stop) {
            return Ok(profile);
        }
        let next = Instant::now() + options.interval;
        while Instant::now() < next && !done(start, options, stop) && crate::process_exists(pid) {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
}
struct LogReader {
    file: std::fs::File,
    tracker: NativeHeapTracker,
    pid: u32,
    map_path: std::path::PathBuf,
    initial_frees: usize,
}
impl LogReader {
    fn open(path: &Path, pid: u32) -> Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let mut header = [0u8; 32];
        file.read_exact(&mut header)?;
        ensure!(
            &header[..8] == b"PPHEAP01" && word(&header, 8) == 1,
            "unsupported native allocation log"
        );
        ensure!(
            word(&header, 16) == pid as u64,
            "allocation log belongs to another PID"
        );
        let metadata = file.metadata()?;
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.uid() == unsafe { libc::getuid() } && metadata.mode() & 0o077 == 0,
            "allocation log must be private and owned by the current user"
        );
        Ok(Self {
            file,
            tracker: NativeHeapTracker::new("native"),
            pid,
            map_path: std::path::PathBuf::from(format!("{}.maps", path.display())),
            initial_frees: 0,
        })
    }
    fn snapshot(&mut self, final_read: bool) -> Result<HeapProfile> {
        let offset = self.file.stream_position()?;
        self.file.seek(SeekFrom::Start(24))?;
        let mut status = [0; 8];
        self.file.read_exact(&mut status)?;
        ensure!(
            u64::from_le_bytes(status) != 3,
            "target exec replaced the allocation recorder; profile the final executable directly"
        );
        ensure!(
            u64::from_le_bytes(status) != 1,
            "native heap recorder lost events due to an I/O failure"
        );
        ensure!(
            matches!(u64::from_le_bytes(status), 0 | 2),
            "invalid native heap recorder status"
        );
        self.file.seek(SeekFrom::Start(offset))?;
        const RECORD: usize = 69 * 8;
        let available = self.file.metadata()?.len().saturating_sub(offset);
        ensure!(
            !final_read || available % RECORD as u64 == 0,
            "target exited during an allocation record; heap capture is incomplete"
        );
        for _ in 0..available / RECORD as u64 {
            let mut event = [0; RECORD];
            self.file.read_exact(&mut event)?;
            let kind = word(&event, 0);
            let address = word(&event, 8);
            let old = word(&event, 16);
            let size = word(&event, 24);
            let depth = word(&event, 32);
            ensure!(depth <= 64, "corrupt allocation stack depth");
            if (kind == 2 || (kind == 3 && old != 0))
                && self
                    .tracker
                    .release(if kind == 2 { address } else { old })
                    .is_err()
            {
                self.initial_frees += 1;
            }
            if (kind == 1 || kind == 3) && address != 0 {
                let stack = (0..depth as usize)
                    .map(|i| HeapFrame {
                        address: word(&event, 40 + i * 8),
                        ..Default::default()
                    })
                    .collect::<Vec<_>>();
                self.tracker.allocate(
                    address,
                    size,
                    if stack.is_empty() {
                        vec![HeapFrame {
                            function: "[unavailable allocation stack]".into(),
                            ..Default::default()
                        }]
                    } else {
                        stack
                    },
                )?;
            }
            ensure!((1..=3).contains(&kind), "unknown native allocation record");
        }
        let mut profile = self.tracker.snapshot()?;
        let maps = std::fs::read_to_string(format!("/proc/{}/maps", self.pid))
            .or_else(|_| std::fs::read_to_string(&self.map_path))
            .context("reading recorded binary mappings")?;
        let mappings = native_mappings(&maps);
        let images = mappings
            .iter()
            .map(|m| {
                (
                    m.memory_start.saturating_sub(m.file_offset),
                    m.filename.clone(),
                )
            })
            .collect::<HashSet<_>>()
            .into_iter()
            .map(|(load_address, path)| LoadedImage { load_address, path })
            .collect();
        let addresses = profile
            .samples
            .keys()
            .flat_map(|s| s.iter().map(|f| f.address))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let symbolizer = NativeSymbolizer::new(images, &addresses);
        profile.samples = profile
            .samples
            .into_iter()
            .map(|(stack, values)| {
                let stack = stack
                    .into_iter()
                    .map(|mut frame| {
                        frame.mapping = mappings
                            .iter()
                            .find(|m| {
                                frame.address >= m.memory_start && frame.address < m.memory_limit
                            })
                            .cloned();
                        if let Some(info) = symbolizer.symbolize_frame(frame.address) {
                            frame.function = info.function;
                            frame.file = info.file;
                            frame.line = info.line as i64;
                        }
                        frame
                    })
                    .collect();
                (stack, values)
            })
            .collect();
        profile.comment = format!(
            "Exact glibc system-allocator capture from recorder initialization. Bytes are malloc_usable_size block sizes. {} frees preceded recorded allocation history. Forked children and custom allocators are excluded.",
            self.initial_frees
        );
        Ok(profile)
    }
}
fn word(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
fn native_mappings(maps: &str) -> Vec<HeapMapping> {
    use object::Object;
    let mut build_ids = std::collections::HashMap::new();
    let mut images = Vec::new();
    for line in maps.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 || !fields[5].starts_with('/') {
            continue;
        }
        let Some((start, end)) = fields[0].split_once('-') else {
            continue;
        };
        let (Ok(start), Ok(end), Ok(offset)) = (
            u64::from_str_radix(start, 16),
            u64::from_str_radix(end, 16),
            u64::from_str_radix(fields[2], 16),
        ) else {
            continue;
        };
        let path = fields[5..]
            .join(" ")
            .trim_end_matches(" (deleted)")
            .to_owned();
        let build_id = build_ids
            .entry(path.clone())
            .or_insert_with(|| {
                std::fs::File::open(&path)
                    .ok()
                    .and_then(|file| unsafe { memmap2::Mmap::map(&file) }.ok())
                    .and_then(|bytes| {
                        object::File::parse(&*bytes)
                            .ok()?
                            .build_id()
                            .ok()?
                            .map(|id| id.iter().map(|b| format!("{b:02x}")).collect::<String>())
                    })
                    .unwrap_or_default()
            })
            .clone();
        images.push(HeapMapping {
            memory_start: start,
            memory_limit: end,
            file_offset: offset,
            filename: path,
            build_id,
        });
    }
    images
}
