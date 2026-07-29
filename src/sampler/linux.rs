//! Linux sampling profiler using `ptrace`, procfs, and `perf_event_open`.
//!
//! Each sampling pass briefly attaches to the selected target threads, reads
//! their registers and frame-pointer chains, then immediately detaches. This
//! keeps the implementation dependency-free and works for child processes
//! under the default Linux ptrace policy. Attaching to an unrelated process
//! may require `CAP_SYS_PTRACE` or a less restrictive Yama configuration.
//! In-process profiling uses per-thread software CPU-clock perf events.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering, fence};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use super::{LoadedImage, RawProfile, RawSampleSeries, ThreadFilter, ThreadSample};

pub struct LinuxSampler {
    pid: u32,
    freq_hz: u32,
    is_self: bool,
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
            is_self: false,
            thread_filter: ThreadFilter::All,
        })
    }

    pub fn new_self(freq_hz: u32) -> Result<Self> {
        let tid = current_tid();
        PerfEvent::open(tid, freq_hz).with_context(|| {
            "opening a Linux perf event for the current process; \
             check /proc/sys/kernel/perf_event_paranoid or grant CAP_PERFMON"
        })?;
        Ok(Self {
            pid: std::process::id(),
            freq_hz,
            is_self: true,
            thread_filter: ThreadFilter::All,
        })
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
        check_child_exit: Option<Box<dyn FnMut() -> bool + Send>>,
        deadline: Option<Instant>,
        live: Arc<Mutex<RawProfile>>,
    ) -> Result<RawProfile> {
        if self.is_self {
            self.run_perf_sampling_loop(stop, deadline, live)
        } else {
            self.run_ptrace_sampling_loop(stop, check_child_exit, deadline, live)
        }
    }

    fn run_ptrace_sampling_loop(
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

    fn run_perf_sampling_loop(
        &self,
        stop: Arc<AtomicBool>,
        deadline: Option<Instant>,
        live: Arc<Mutex<RawProfile>>,
    ) -> Result<RawProfile> {
        let start_time = live.lock().unwrap().start_time;
        let sampler_tid = current_tid();
        let threads = self.selected_threads(Some(sampler_tid))?;
        let mut events = Vec::new();
        let mut first_error = None;
        for (tid, name) in threads {
            match PerfEvent::open(tid, self.freq_hz) {
                Ok(event) => events.push((tid, name, event)),
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            };
        }
        if events.is_empty() {
            return Err(first_error.unwrap_or_else(|| {
                anyhow::anyhow!("no threads matched the configured Linux thread filter")
            }));
        }

        for (_, _, event) in &events {
            event.enable()?;
        }

        let poll_interval = Duration::from_micros(
            (1_000_000 / self.freq_hz as u64)
                .clamp(500, Duration::from_millis(5).as_micros() as u64),
        );
        loop {
            if stop.load(Ordering::Relaxed) || deadline.is_some_and(|value| Instant::now() >= value)
            {
                break;
            }
            self.drain_perf_events(&mut events, start_time, &live);
            std::thread::sleep(poll_interval);
        }

        for (_, _, event) in &events {
            let _ = event.disable();
        }
        self.drain_perf_events(&mut events, start_time, &live);
        stop.store(true, Ordering::Relaxed);

        let images = self.read_loaded_images().unwrap_or_default();
        let mut current = live.lock().unwrap();
        current.end_time = Instant::now();
        if !images.is_empty() {
            current.images = images;
        }
        Ok(current.clone())
    }

    fn drain_perf_events(
        &self,
        events: &mut [(u32, String, PerfEvent)],
        start_time: Instant,
        live: &Arc<Mutex<RawProfile>>,
    ) {
        let timestamp_nanos = Instant::now().duration_since(start_time).as_nanos() as u64;
        let mut current = live.lock().unwrap();
        for (configured_tid, name, event) in events {
            for sample in event.drain_samples() {
                let tid = if sample.thread_id == 0 {
                    *configured_tid
                } else {
                    sample.thread_id
                };
                if sample.stack.is_empty() {
                    continue;
                }
                current
                    .stacks
                    .entry((tid as u64, sample.stack))
                    .or_insert_with(|| RawSampleSeries::timed(Vec::new()))
                    .push_timestamp(timestamp_nanos);
                if !name.is_empty() {
                    current.thread_names.insert(tid as u64, name.clone());
                }
            }
        }
        current.end_time = Instant::now();
    }

    pub fn read_loaded_images(&self) -> Result<Vec<LoadedImage>> {
        let maps = std::fs::read_to_string(format!("/proc/{}/maps", self.pid))
            .context("reading process memory maps")?;
        Ok(parse_loaded_images(&maps))
    }

    fn sample_once(&self, start_time: Instant) -> Vec<ThreadSample> {
        let Ok(threads) = self.selected_threads(None) else {
            return Vec::new();
        };

        let mut samples = Vec::new();
        for (tid, name) in threads {
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

    fn selected_threads(&self, skip_tid: Option<u32>) -> Result<Vec<(u32, String)>> {
        let mut threads = list_threads(self.pid)?;
        threads.sort_unstable();
        let mut selected = Vec::new();
        for tid in threads.into_iter().filter(|tid| Some(*tid) != skip_tid) {
            let name = thread_name(self.pid, tid);
            let index = selected.len();
            if self.thread_matches_filter(tid, &name, index) {
                selected.push((tid, name));
            }
        }
        Ok(selected)
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

// ---------------------------------------------------------------------------
// In-process sampling via perf_event_open
// ---------------------------------------------------------------------------

const PERF_TYPE_SOFTWARE: u32 = 1;
const PERF_COUNT_SW_CPU_CLOCK: u64 = 0;
const PERF_SAMPLE_IP: u64 = 1 << 0;
const PERF_SAMPLE_TID: u64 = 1 << 1;
const PERF_SAMPLE_CALLCHAIN: u64 = 1 << 5;
const PERF_RECORD_SAMPLE: u32 = 9;
const PERF_FLAG_FD_CLOEXEC: u64 = 1 << 3;
const PERF_EVENT_IOC_ENABLE: libc::c_ulong = 0x2400;
const PERF_EVENT_IOC_DISABLE: libc::c_ulong = 0x2401;
const PERF_EVENT_IOC_RESET: libc::c_ulong = 0x2403;
const PERF_MMAP_DATA_HEAD: usize = 1024;
const PERF_MMAP_DATA_TAIL: usize = 1032;
const PERF_MMAP_DATA_OFFSET: usize = 1040;
const PERF_MMAP_DATA_SIZE: usize = 1048;
const PERF_DATA_PAGES: usize = 8;
const PERF_CONTEXT_MARKER_MIN: u64 = u64::MAX - 4095;

#[repr(C)]
struct PerfEventAttr {
    type_: u32,
    size: u32,
    config: u64,
    sample_freq: u64,
    sample_type: u64,
    read_format: u64,
    flags: u64,
    wakeup_events: u32,
    bp_type: u32,
    config1: u64,
}

impl PerfEventAttr {
    fn software_cpu_clock(freq_hz: u32) -> Self {
        // perf_event_attr bitfield values from linux/perf_event.h:
        // disabled=bit 0, exclude_kernel=bit 5, exclude_hv=bit 6,
        // freq=bit 10.
        let flags = (1 << 0) | (1 << 5) | (1 << 6) | (1 << 10);
        Self {
            type_: PERF_TYPE_SOFTWARE,
            size: std::mem::size_of::<Self>() as u32,
            config: PERF_COUNT_SW_CPU_CLOCK,
            sample_freq: freq_hz as u64,
            sample_type: PERF_SAMPLE_IP | PERF_SAMPLE_TID | PERF_SAMPLE_CALLCHAIN,
            read_format: 0,
            flags,
            wakeup_events: 1,
            bp_type: 0,
            config1: 0,
        }
    }
}

struct PerfSample {
    thread_id: u32,
    stack: Vec<u64>,
}

struct PerfEvent {
    fd: libc::c_int,
    mapping: *mut u8,
    mapping_len: usize,
    data_offset: usize,
    data_size: usize,
    tail: u64,
}

// The mapping and file descriptor are owned by PerfEvent and only accessed
// from the sampler thread after construction.
unsafe impl Send for PerfEvent {}

impl PerfEvent {
    fn open(tid: u32, freq_hz: u32) -> Result<Self> {
        let attr = PerfEventAttr::software_cpu_clock(freq_hz);
        let fd = unsafe {
            libc::syscall(
                libc::SYS_perf_event_open,
                &attr as *const PerfEventAttr,
                tid as libc::pid_t,
                -1i32,
                -1i32,
                PERF_FLAG_FD_CLOEXEC,
            ) as libc::c_int
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error()).context("perf_event_open");
        }

        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if page_size <= 0 {
            unsafe { libc::close(fd) };
            bail!("sysconf(_SC_PAGESIZE) failed");
        }
        let page_size = page_size as usize;
        let mapping_len = page_size * (1 + PERF_DATA_PAGES);
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                mapping_len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if mapping == libc::MAP_FAILED {
            let error = std::io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(error).context("mmap perf event ring");
        }
        let mapping = mapping.cast::<u8>();
        let kernel_data_offset =
            unsafe { std::ptr::read_volatile(mapping.add(PERF_MMAP_DATA_OFFSET).cast::<u64>()) };
        let kernel_data_size =
            unsafe { std::ptr::read_volatile(mapping.add(PERF_MMAP_DATA_SIZE).cast::<u64>()) };
        let data_offset = if kernel_data_offset == 0 {
            page_size
        } else {
            kernel_data_offset as usize
        };
        let data_size = if kernel_data_size == 0 {
            mapping_len - page_size
        } else {
            kernel_data_size as usize
        };
        if data_offset
            .checked_add(data_size)
            .is_none_or(|end| end > mapping_len)
        {
            unsafe {
                libc::munmap(mapping.cast(), mapping_len);
                libc::close(fd);
            }
            bail!("kernel returned an invalid perf event ring layout");
        }
        let tail =
            unsafe { std::ptr::read_volatile(mapping.add(PERF_MMAP_DATA_TAIL).cast::<u64>()) };
        Ok(Self {
            fd,
            mapping,
            mapping_len,
            data_offset,
            data_size,
            tail,
        })
    }

    fn enable(&self) -> Result<()> {
        self.ioctl(PERF_EVENT_IOC_RESET)
            .context("resetting perf event")?;
        self.ioctl(PERF_EVENT_IOC_ENABLE)
            .context("enabling perf event")
    }

    fn disable(&self) -> Result<()> {
        self.ioctl(PERF_EVENT_IOC_DISABLE)
            .context("disabling perf event")
    }

    fn ioctl(&self, request: libc::c_ulong) -> Result<()> {
        let result = unsafe { libc::ioctl(self.fd, request, 0) };
        if result == -1 {
            Err(std::io::Error::last_os_error().into())
        } else {
            Ok(())
        }
    }

    fn drain_samples(&mut self) -> Vec<PerfSample> {
        let head =
            unsafe { std::ptr::read_volatile(self.mapping.add(PERF_MMAP_DATA_HEAD).cast::<u64>()) };
        fence(Ordering::Acquire);
        if head.saturating_sub(self.tail) > self.data_size as u64 {
            self.tail = head - self.data_size as u64;
        }

        let mut samples = Vec::new();
        while head.saturating_sub(self.tail) >= 8 {
            let header = self.copy_ring(self.tail, 8);
            let record_type = u32::from_ne_bytes(header[0..4].try_into().unwrap());
            let record_size = u16::from_ne_bytes(header[6..8].try_into().unwrap()) as usize;
            if record_size < 8 || record_size as u64 > head.saturating_sub(self.tail) {
                break;
            }
            let record = self.copy_ring(self.tail, record_size);
            self.tail += record_size as u64;
            if record_type == PERF_RECORD_SAMPLE
                && let Some(sample) = parse_perf_sample(&record)
            {
                samples.push(sample);
            }
        }

        fence(Ordering::Release);
        unsafe {
            std::ptr::write_volatile(
                self.mapping.add(PERF_MMAP_DATA_TAIL).cast::<u64>(),
                self.tail,
            );
        }
        samples
    }

    fn copy_ring(&self, position: u64, length: usize) -> Vec<u8> {
        let start = position as usize % self.data_size;
        let first_length = length.min(self.data_size - start);
        let mut output = Vec::with_capacity(length);
        unsafe {
            output.extend_from_slice(std::slice::from_raw_parts(
                self.mapping.add(self.data_offset + start),
                first_length,
            ));
            if first_length < length {
                output.extend_from_slice(std::slice::from_raw_parts(
                    self.mapping.add(self.data_offset),
                    length - first_length,
                ));
            }
        }
        output
    }
}

impl Drop for PerfEvent {
    fn drop(&mut self) {
        let _ = self.disable();
        unsafe {
            libc::munmap(self.mapping.cast(), self.mapping_len);
            libc::close(self.fd);
        }
    }
}

fn parse_perf_sample(record: &[u8]) -> Option<PerfSample> {
    let mut offset = 8;
    let instruction_pointer = take_u64(record, &mut offset)?;
    let _process_id = take_u32(record, &mut offset)?;
    let thread_id = take_u32(record, &mut offset)?;
    let frame_count = take_u64(record, &mut offset)? as usize;
    if frame_count > 4096 {
        return None;
    }

    let mut stack = Vec::with_capacity(frame_count.saturating_add(1));
    for _ in 0..frame_count {
        let address = take_u64(record, &mut offset)?;
        if address != 0 && address < PERF_CONTEXT_MARKER_MIN {
            stack.push(address);
        }
    }
    if instruction_pointer != 0 && stack.first().copied() != Some(instruction_pointer) {
        stack.insert(0, instruction_pointer);
    }
    Some(PerfSample { thread_id, stack })
}

fn take_u64(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let value = u64::from_ne_bytes(bytes.get(*offset..*offset + 8)?.try_into().ok()?);
    *offset += 8;
    Some(value)
}

fn take_u32(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    let value = u32::from_ne_bytes(bytes.get(*offset..*offset + 4)?.try_into().ok()?);
    *offset += 4;
    Some(value)
}

fn current_tid() -> u32 {
    unsafe { libc::syscall(libc::SYS_gettid) as u32 }
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
    fn perf_event_attr_uses_the_version_zero_abi_size() {
        let attr = PerfEventAttr::software_cpu_clock(99);
        assert_eq!(std::mem::size_of::<PerfEventAttr>(), 64);
        assert_eq!(attr.size, 64);
    }

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
