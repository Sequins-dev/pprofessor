//! Linux sampling profiler using `ptrace` and procfs.
//!
//! Each sampling pass briefly attaches to the selected target threads, reads
//! their registers and frame-pointer chains, then immediately detaches. This
//! keeps the implementation dependency-free and works for child processes
//! under the default Linux ptrace policy. Attaching to an unrelated process
//! may require `CAP_SYS_PTRACE` or a less restrictive Yama configuration.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use super::{LoadedImage, RawProfile, RawSampleSeries, ThreadFilter, ThreadSample};

pub struct LinuxSampler {
    pid: u32,
    freq_hz: u32,
    pub thread_filter: ThreadFilter,
}

impl LinuxSampler {
    pub fn new(pid: u32, freq_hz: u32) -> Result<Self> {
        if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            bail!("process {pid} does not exist");
        }
        probe_ptrace(pid as libc::pid_t).with_context(|| {
            format!(
                "ptrace attach to pid {pid} failed; Linux ptrace policy may require \
                 CAP_SYS_PTRACE or a parent-child relationship"
            )
        })?;
        Ok(Self {
            pid,
            freq_hz,
            thread_filter: ThreadFilter::All,
        })
    }

    pub fn new_self(_freq_hz: u32) -> Result<Self> {
        bail!("in-process profiling is not supported on Linux")
    }

    pub fn spawn(
        cmd: &mut std::process::Command,
        freq_hz: u32,
    ) -> Result<(std::process::Child, Self)> {
        let mut child = cmd.spawn().context("spawning child process")?;
        let pid = child.id();
        match Self::new(pid, freq_hz) {
            Ok(sampler) => Ok((child, sampler)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }

    pub fn run_sampling_loop(
        &self,
        stop: Arc<AtomicBool>,
        mut check_child_exit: Option<Box<dyn FnMut() -> bool + Send>>,
        deadline: Option<Instant>,
        live: Arc<Mutex<RawProfile>>,
    ) -> Result<RawProfile> {
        let interval = Duration::from_micros(1_000_000 / self.freq_hz as u64);
        let start_time = live.lock().unwrap().start_time;
        let mut next_image_refresh = Instant::now();

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if let Some(ref mut check) = check_child_exit
                && check()
            {
                break;
            }
            if deadline.is_some_and(|value| Instant::now() >= value) {
                break;
            }

            if Instant::now() >= next_image_refresh {
                if let Ok(images) = self.read_loaded_images()
                    && !images.is_empty()
                {
                    live.lock().unwrap().images = images;
                }
                next_image_refresh = Instant::now() + Duration::from_millis(500);
            }

            let round = self.sample_once(start_time);
            {
                let mut current = live.lock().unwrap();
                for sample in round {
                    current
                        .stacks
                        .entry((sample.thread_id, sample.stack))
                        .or_insert_with(|| RawSampleSeries::timed(Vec::new()))
                        .push_timestamp(sample.timestamp_nanos);
                    if !sample.thread_name.is_empty() {
                        current
                            .thread_names
                            .insert(sample.thread_id, sample.thread_name);
                    }
                }
                current.end_time = Instant::now();
            }

            std::thread::sleep(interval);
        }

        stop.store(true, Ordering::Relaxed);
        let images = self.read_loaded_images().unwrap_or_default();
        let mut current = live.lock().unwrap();
        current.end_time = Instant::now();
        if !images.is_empty() {
            current.images = images;
        }
        Ok(current.clone())
    }

    pub fn read_loaded_images(&self) -> Result<Vec<LoadedImage>> {
        let maps = std::fs::read_to_string(format!("/proc/{}/maps", self.pid))
            .context("reading process memory maps")?;
        Ok(parse_loaded_images(&maps))
    }

    fn sample_once(&self, start_time: Instant) -> Vec<ThreadSample> {
        let Ok(mut threads) = list_threads(self.pid) else {
            return Vec::new();
        };
        threads.sort_unstable();

        let mut samples = Vec::new();
        for (index, tid) in threads.into_iter().enumerate() {
            let name = thread_name(self.pid, tid);
            if !self.thread_matches_filter(tid, &name, index) {
                continue;
            }
            if let Some(stack) = sample_thread(tid) {
                samples.push(ThreadSample {
                    thread_id: tid as u64,
                    thread_name: name,
                    stack,
                    timestamp_nanos: Instant::now().duration_since(start_time).as_nanos() as u64,
                });
            }
        }
        samples
    }

    fn thread_matches_filter(&self, tid: u32, name: &str, index: usize) -> bool {
        match &self.thread_filter {
            ThreadFilter::All => true,
            ThreadFilter::MainThread => index == 0,
            ThreadFilter::ByName(expected) => name.contains(expected),
            ThreadFilter::ById(expected) => tid as u64 == *expected,
            ThreadFilter::ByMachThread(_) => false,
        }
    }
}

struct AttachedThread(libc::pid_t);

impl AttachedThread {
    fn attach(tid: libc::pid_t) -> Result<Self> {
        let result = unsafe {
            libc::ptrace(
                libc::PTRACE_ATTACH as _,
                tid,
                std::ptr::null_mut::<c_void>(),
                std::ptr::null_mut::<c_void>(),
            )
        };
        if result == -1 {
            return Err(std::io::Error::last_os_error().into());
        }

        let mut status = 0;
        let waited = unsafe { libc::waitpid(tid, &mut status, libc::__WALL) };
        if waited != tid || !libc::WIFSTOPPED(status) {
            unsafe {
                libc::ptrace(
                    libc::PTRACE_DETACH as _,
                    tid,
                    std::ptr::null_mut::<c_void>(),
                    std::ptr::null_mut::<c_void>(),
                );
            }
            bail!("thread {tid} did not stop after ptrace attach");
        }
        Ok(Self(tid))
    }
}

impl Drop for AttachedThread {
    fn drop(&mut self) {
        unsafe {
            libc::ptrace(
                libc::PTRACE_DETACH as _,
                self.0,
                std::ptr::null_mut::<c_void>(),
                std::ptr::null_mut::<c_void>(),
            );
        }
    }
}

fn probe_ptrace(tid: libc::pid_t) -> Result<()> {
    let _attached = AttachedThread::attach(tid)?;
    Ok(())
}

fn sample_thread(tid: u32) -> Option<Vec<u64>> {
    let _attached = AttachedThread::attach(tid as libc::pid_t).ok()?;
    let (pc, fp) = read_registers(tid as libc::pid_t)?;
    let mut stack = vec![pc];
    walk_frame_pointers(tid as libc::pid_t, fp, &mut stack);
    Some(stack)
}

fn read_registers(tid: libc::pid_t) -> Option<(u64, u64)> {
    let mut registers = std::mem::MaybeUninit::<libc::user_regs_struct>::zeroed();
    let mut registers_iovec = libc::iovec {
        iov_base: registers.as_mut_ptr().cast(),
        iov_len: std::mem::size_of::<libc::user_regs_struct>(),
    };
    let result = unsafe {
        libc::ptrace(
            libc::PTRACE_GETREGSET as _,
            tid,
            libc::NT_PRSTATUS as usize as *mut c_void,
            &mut registers_iovec as *mut libc::iovec,
        )
    };
    if result == -1 {
        return None;
    }
    let registers = unsafe { registers.assume_init() };

    #[cfg(target_arch = "x86_64")]
    return Some((registers.rip, registers.rbp));

    #[cfg(target_arch = "aarch64")]
    return Some((registers.pc, registers.regs[29]));

    #[allow(unreachable_code)]
    None
}

fn walk_frame_pointers(tid: libc::pid_t, mut fp: u64, stack: &mut Vec<u64>) {
    const MAX_DEPTH: usize = 256;
    for _ in 0..MAX_DEPTH {
        if fp == 0 || fp & 0x7 != 0 {
            break;
        }
        let mut frame = [0u8; 16];
        if !read_process_memory(tid, fp, &mut frame) {
            break;
        }
        let previous_fp = u64::from_ne_bytes(frame[0..8].try_into().unwrap());
        let return_address = u64::from_ne_bytes(frame[8..16].try_into().unwrap());
        if return_address == 0 {
            break;
        }
        stack.push(return_address);
        if previous_fp <= fp {
            break;
        }
        fp = previous_fp;
    }
}

fn read_process_memory(tid: libc::pid_t, address: u64, output: &mut [u8]) -> bool {
    let local = libc::iovec {
        iov_base: output.as_mut_ptr().cast(),
        iov_len: output.len(),
    };
    let remote = libc::iovec {
        iov_base: address as usize as *mut c_void,
        iov_len: output.len(),
    };
    let count = unsafe { libc::process_vm_readv(tid, &local, 1, &remote, 1, 0) };
    count == output.len() as isize
}

fn list_threads(pid: u32) -> Result<Vec<u32>> {
    let entries = std::fs::read_dir(format!("/proc/{pid}/task"))?;
    Ok(entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_string_lossy().parse().ok())
        .collect())
}

fn thread_name(pid: u32, tid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/task/{tid}/comm"))
        .map(|name| name.trim_end().to_string())
        .unwrap_or_default()
}

fn parse_loaded_images(maps: &str) -> Vec<LoadedImage> {
    let mut images = BTreeSet::new();
    for line in maps.lines() {
        let mut fields = line.split_whitespace();
        let Some(range) = fields.next() else {
            continue;
        };
        let _permissions = fields.next();
        let Some(offset) = fields.next().and_then(parse_hex) else {
            continue;
        };
        let _device = fields.next();
        let _inode = fields.next();
        let path = fields.collect::<Vec<_>>().join(" ");
        if !path.starts_with('/') || path.ends_with(" (deleted)") {
            continue;
        }
        let Some(start) = range
            .split_once('-')
            .and_then(|(start, _)| parse_hex(start))
        else {
            continue;
        };
        if let Some(load_address) = start.checked_sub(offset) {
            images.insert((load_address, unescape_proc_path(&path)));
        }
    }
    images
        .into_iter()
        .map(|(load_address, path)| LoadedImage { load_address, path })
        .collect()
}

fn parse_hex(value: &str) -> Option<u64> {
    u64::from_str_radix(value, 16).ok()
}

fn unescape_proc_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1..index + 4]
                .iter()
                .all(|byte| matches!(byte, b'0'..=b'7'))
        {
            let value = (bytes[index + 1] - b'0') * 64
                + (bytes[index + 2] - b'0') * 8
                + (bytes[index + 3] - b'0');
            output.push(value);
            index += 4;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_maps_are_collapsed_to_one_image_per_load_bias() {
        let maps = "\
55555000-55556000 r--p 00000000 08:01 1 /tmp/test\\040binary\n\
55556000-55557000 r-xp 00001000 08:01 1 /tmp/test\\040binary\n\
7f000000-7f001000 r--p 00000000 08:01 2 /usr/lib/libc.so.6\n";

        assert_eq!(
            parse_loaded_images(maps),
            vec![
                LoadedImage {
                    load_address: 0x55555000,
                    path: "/tmp/test binary".to_string(),
                },
                LoadedImage {
                    load_address: 0x7f000000,
                    path: "/usr/lib/libc.so.6".to_string(),
                },
            ]
        );
    }
}
