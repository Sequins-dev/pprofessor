//! macOS sampling profiler using Mach kernel APIs.
//!
//! ## Permissions
//!
//! Getting a task port for another process requires either:
//! - Running as root (`sudo pprofessor ...`)
//! - The binary being signed with the `com.apple.security.cs.debugger` entitlement
//!
//! To self-sign for development (no Apple Developer account needed):
//! ```sh
//! codesign --force --entitlements entitlements.plist --sign - ./target/release/pprofessor
//! ```
//! where `entitlements.plist` sets `com.apple.security.cs.debugger = true`.
//! The `make sign` target in the Makefile does this automatically.
//!
//! ## Sampling algorithm
//! 1. Obtain a Mach task port via `task_for_pid`.
//! 2. At the configured frequency, enumerate threads with `task_threads`.
//! 3. For each thread: suspend → read register state → walk frame pointers → resume.
//! 4. Accumulate stacks keyed by their instruction-pointer sequence.
//! 5. At shutdown, read the target's dyld image list for symbolication.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use libc::pid_t;
use mach2::kern_return::KERN_SUCCESS;
use mach2::mach_types::{task_t, thread_act_array_t, thread_act_t};
use mach2::message::mach_msg_type_number_t;
use mach2::port::mach_port_t;
use mach2::thread_act::{thread_resume, thread_suspend};
use mach2::vm_types::mach_vm_address_t;

use super::{LoadedImage, RawProfile, RawSampleSeries, ThreadFilter, ThreadSample};

// ---------------------------------------------------------------------------
// FFI: functions not yet bound by mach2
// ---------------------------------------------------------------------------

type KernReturn = i32;

unsafe extern "C" {
    fn task_for_pid(target_tport: mach_port_t, pid: pid_t, t: *mut task_t) -> KernReturn;

    fn mach_task_self() -> mach_port_t;

    fn mach_thread_self() -> mach_port_t;

    fn task_threads(
        target_task: task_t,
        act_list: *mut thread_act_array_t,
        act_list_cnt: *mut mach_msg_type_number_t,
    ) -> KernReturn;

    fn mach_vm_read_overwrite(
        target_task: task_t,
        address: mach_vm_address_t,
        size: u64,
        data: mach_vm_address_t,
        out_size: *mut u64,
    ) -> KernReturn;

    fn mach_vm_deallocate(target: task_t, address: mach_vm_address_t, size: u64) -> KernReturn;

    fn mach_port_deallocate(task: mach_port_t, name: mach_port_t) -> KernReturn;

    fn task_info(
        target_task: task_t,
        flavor: u32,
        task_info_out: *mut libc::c_int,
        task_info_out_cnt: *mut mach_msg_type_number_t,
    ) -> KernReturn;

    fn thread_get_state(
        thread: thread_act_t,
        flavor: i32,
        old_state: *mut libc::c_void,
        old_state_cnt: *mut mach_msg_type_number_t,
    ) -> KernReturn;

    fn thread_info(
        thread: thread_act_t,
        flavor: u32,
        thread_info_out: *mut libc::c_int,
        thread_info_out_cnt: *mut mach_msg_type_number_t,
    ) -> KernReturn;

}

// ---------------------------------------------------------------------------
// Thread identifier info (cross-architecture, always available)
// ---------------------------------------------------------------------------

const THREAD_IDENTIFIER_INFO_FLAVOR: u32 = 4;
const THREAD_IDENTIFIER_INFO_COUNT: u32 = 6; // sizeof(thread_identifier_info_data_t) / sizeof(natural_t)

#[repr(C)]
struct ThreadIdentifierInfo {
    thread_id: u64,
    thread_handle: u64,
    dispatch_qaddr: u64,
}

// ---------------------------------------------------------------------------
// Thread state structs (architecture-specific)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "aarch64")]
mod arch {
    pub const THREAD_STATE_FLAVOR: i32 = 6; // ARM_THREAD_STATE64
    pub const THREAD_STATE_COUNT: u32 = 68; // sizeof(arm_thread_state64_t) / 4

    #[repr(C)]
    pub struct ThreadState {
        pub x: [u64; 29], // x0–x28
        pub fp: u64,      // x29 (frame pointer)
        pub lr: u64,      // x30 (link register)
        pub sp: u64,
        pub pc: u64,
        pub cpsr: u32,
        pub _pad: u32,
    }

    /// Strip pointer authentication codes from an address on Apple Silicon.
    /// The virtual address space on current Apple Silicon uses at most 39 bits.
    pub fn strip_pac(addr: u64) -> u64 {
        addr & 0x0000_007F_FFFF_FFFF
    }
}

#[cfg(target_arch = "x86_64")]
mod arch {
    pub const THREAD_STATE_FLAVOR: i32 = 4; // x86_THREAD_STATE64
    pub const THREAD_STATE_COUNT: u32 = 42; // sizeof(x86_thread_state64_t) / 4

    #[repr(C)]
    pub struct ThreadState {
        pub rax: u64,
        pub rbx: u64,
        pub rcx: u64,
        pub rdx: u64,
        pub rdi: u64,
        pub rsi: u64,
        pub rbp: u64, // frame pointer
        pub rsp: u64,
        pub r8: u64,
        pub r9: u64,
        pub r10: u64,
        pub r11: u64,
        pub r12: u64,
        pub r13: u64,
        pub r14: u64,
        pub r15: u64,
        pub rip: u64,
        pub rflags: u64,
        pub cs: u64,
        pub fs: u64,
        pub gs: u64,
    }

    pub fn strip_pac(addr: u64) -> u64 {
        addr // no PAC on x86_64
    }
}

use arch::{THREAD_STATE_COUNT, THREAD_STATE_FLAVOR, ThreadState, strip_pac};

// ---------------------------------------------------------------------------
// dyld image info structs for reading the target's loaded library list
// ---------------------------------------------------------------------------

const TASK_DYLD_INFO: u32 = 17;
const TASK_DYLD_INFO_COUNT: u32 = 5;

#[repr(C)]
struct TaskDyldInfo {
    all_image_info_addr: u64,
    all_image_info_size: u64,
    all_image_info_format: i32,
    _pad: i32,
}

// Matches the prefix of struct dyld_all_image_infos through its version 2 fields.
#[repr(C)]
struct DyldAllImageInfos {
    version: u32,
    info_array_count: u32,
    info_array: u64, // pointer to array of dyld_image_info in the target
    notification: u64,
    process_detached_from_shared_region: u8,
    lib_system_initialized: u8,
    dyld_image_load_address: u64,
}

// Matches struct dyld_image_info.
#[repr(C)]
struct DyldImageInfo {
    image_load_address: u64, // pointer to mach_header in the target
    image_file_path: u64,    // pointer to C string in the target
    image_file_mod_date: u64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn mach_check(kr: KernReturn, ctx: &str) -> Result<()> {
    if kr == KERN_SUCCESS {
        Ok(())
    } else {
        bail!("{ctx}: mach error {kr}")
    }
}

/// Read bytes from the target process's address space.
fn read_target_mem(task: task_t, addr: u64, buf: &mut [u8]) -> Result<()> {
    let mut out_size: u64 = 0;
    let kr = unsafe {
        mach_vm_read_overwrite(
            task,
            addr,
            buf.len() as u64,
            buf.as_mut_ptr() as mach_vm_address_t,
            &mut out_size,
        )
    };
    mach_check(kr, "mach_vm_read_overwrite")?;
    if out_size != buf.len() as u64 {
        bail!("short read: expected {} bytes, got {}", buf.len(), out_size);
    }
    Ok(())
}

/// Read a null-terminated C string from the target process's address space.
fn read_target_cstr(task: task_t, addr: u64) -> Result<String> {
    let mut result = Vec::new();
    let mut offset = 0u64;
    loop {
        let mut chunk = [0u8; 256];
        if read_target_mem(task, addr + offset, &mut chunk).is_err() {
            break;
        }
        if let Some(nul) = chunk.iter().position(|&b| b == 0) {
            result.extend_from_slice(&chunk[..nul]);
            break;
        }
        result.extend_from_slice(&chunk);
        offset += 256;
        if offset > 4096 {
            break; // sanity limit
        }
    }
    Ok(String::from_utf8_lossy(&result).into_owned())
}

// ---------------------------------------------------------------------------
// MacosSampler
// ---------------------------------------------------------------------------

pub struct MacosSampler {
    task: task_t,
    freq_hz: u32,
    /// Whether we allocated the task port and must deallocate it on drop.
    owns_task_port: bool,
    /// True when profiling the current process. The sampling thread skips itself
    /// by capturing mach_thread_self() at the start of each run_sampling_loop call.
    is_self: bool,
    executable_path: Option<String>,
    /// Which threads to include in the profile.
    pub thread_filter: ThreadFilter,
}

// task_t is a u32 Mach port name. Mach sampling APIs (task_threads, thread_get_state,
// mach_vm_read_overwrite) are thread-safe for concurrent readers, so MacosSampler is
// safe to share across threads via Arc.
unsafe impl Send for MacosSampler {}
unsafe impl Sync for MacosSampler {}

impl MacosSampler {
    /// Obtain a sampler for the given PID via `task_for_pid`.
    ///
    /// Requires root or the `com.apple.security.cs.debugger` entitlement.
    /// Run `make sign` to self-sign the pprofessor binary for development use.
    pub fn new(pid: u32, freq_hz: u32) -> Result<Self> {
        let mut task: task_t = 0;
        let kr = unsafe { task_for_pid(mach_task_self(), pid as pid_t, &mut task) };
        if kr != KERN_SUCCESS {
            bail!(
                "task_for_pid({pid}) failed (mach error {kr}).\n\
                 The profiler needs the com.apple.security.cs.debugger entitlement. \
                 Hardened targets must also opt into debugging with com.apple.security.get-task-allow."
            );
        }
        Ok(MacosSampler {
            task,
            freq_hz,
            owns_task_port: true,
            is_self: false,
            executable_path: process_executable_path(pid),
            thread_filter: ThreadFilter::All,
        })
    }

    /// Obtain a sampler for the current process.
    ///
    /// Uses `mach_task_self()` which requires no special permissions. The sampling
    /// thread automatically skips itself to prevent deadlock.
    pub fn new_self(freq_hz: u32) -> Result<Self> {
        let task = unsafe { mach_task_self() };
        Ok(MacosSampler {
            task,
            freq_hz,
            owns_task_port: false, // mach_task_self() is a pseudo-port, never deallocate
            is_self: true,
            executable_path: std::env::current_exe()
                .ok()
                .map(|path| path.to_string_lossy().into_owned()),
            thread_filter: ThreadFilter::All,
        })
    }

    /// Spawn a child process, attach a sampler to it, and return both.
    ///
    /// Requires the same permissions as [`new`] (`task_for_pid`).
    pub fn spawn(
        cmd: &mut std::process::Command,
        freq_hz: u32,
    ) -> Result<(std::process::Child, Self)> {
        // Ask the child to pause immediately after exec so we can call task_for_pid
        // while the task is freshly initialised.  Even with PT_TRACE_ME, task_for_pid
        // still requires root or the debugger entitlement on macOS 12+, but it
        // gives us a reliable synchronisation point (we know when the child is
        // ready to be profiled).
        unsafe {
            cmd.pre_exec(|| {
                libc::ptrace(libc::PT_TRACE_ME, 0, std::ptr::null_mut(), 0);
                Ok(())
            });
        }

        let child = cmd.spawn().context("spawning child process")?;
        let child_pid = child.id();

        // Wait for the child's initial stop (SIGTRAP fired after exec).
        let mut status = 0i32;
        unsafe { libc::waitpid(child_pid as pid_t, &mut status, libc::WUNTRACED) };

        // Obtain the task port. On systems where task_for_pid requires privileges,
        // this will fail with a clear error message.
        let sampler = Self::new(child_pid, freq_hz)?;

        // Let the child run.
        unsafe {
            libc::ptrace(
                libc::PT_CONTINUE,
                child_pid as pid_t,
                std::ptr::dangling_mut::<libc::c_char>(),
                0,
            );
        }

        Ok((child, sampler))
    }

    /// Retrieve the stable numeric thread ID (and, for same-process threads, the name)
    /// for a Mach thread port.
    ///
    /// - `thread_id` is always available via `thread_info(THREAD_IDENTIFIER_INFO)`.
    /// - Thread names are only readable in-process; for external targets the name is "".
    fn get_thread_identity(&self, thread: thread_act_t) -> (u64, String) {
        let mut info = ThreadIdentifierInfo {
            thread_id: 0,
            thread_handle: 0,
            dispatch_qaddr: 0,
        };
        let mut count: mach_msg_type_number_t = THREAD_IDENTIFIER_INFO_COUNT;
        let kr = unsafe {
            thread_info(
                thread,
                THREAD_IDENTIFIER_INFO_FLAVOR,
                &mut info as *mut ThreadIdentifierInfo as *mut libc::c_int,
                &mut count,
            )
        };
        let thread_id = if kr == KERN_SUCCESS && info.thread_id != 0 {
            info.thread_id
        } else {
            thread as u64 // fallback: mach port number
        };

        let name = if self.is_self {
            let pthread = unsafe { libc::pthread_from_mach_thread_np(thread) };
            if pthread != 0 {
                let mut buf = [0i8; 64];
                let rc = unsafe { libc::pthread_getname_np(pthread, buf.as_mut_ptr(), buf.len()) };
                if rc == 0 {
                    let name_bytes = buf
                        .iter()
                        .take_while(|&&c| c != 0)
                        .map(|&c| c as u8)
                        .collect::<Vec<_>>();
                    String::from_utf8_lossy(&name_bytes).into_owned()
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        (thread_id, name)
    }

    /// Returns true if `thread` passes the current thread filter.
    /// Must be called after `thread_suspend` so `get_thread_identity` sees stable state.
    /// `index` is the position of this thread in the `task_threads()` array.
    fn thread_matches_filter(
        &self,
        thread: thread_act_t,
        thread_id: u64,
        name: &str,
        index: usize,
    ) -> bool {
        match &self.thread_filter {
            ThreadFilter::All => true,
            ThreadFilter::MainThread => index == 0,
            ThreadFilter::ByName(n) => name.contains(n.as_str()),
            ThreadFilter::ById(id) => thread_id == *id,
            ThreadFilter::ByMachThread(port) => thread == *port,
        }
    }

    /// Collect one round of stack samples from threads in the target.
    ///
    /// `skip_thread`: if provided, skip sampling that thread (used to skip the
    /// sampler's own background thread when profiling the current process).
    fn sample_once(
        &self,
        skip_thread: Option<mach_port_t>,
        start_time: Instant,
    ) -> Vec<ThreadSample> {
        let mut samples = Vec::new();

        let mut thread_list: thread_act_array_t = std::ptr::null_mut();
        let mut thread_count: mach_msg_type_number_t = 0;

        let kr = unsafe { task_threads(self.task, &mut thread_list, &mut thread_count) };
        if kr != KERN_SUCCESS {
            return samples;
        }

        // For MainThread, track the index among non-skipped threads.
        let mut non_skip_index: usize = 0;
        for i in 0..thread_count as usize {
            let thread = unsafe { *thread_list.add(i) };

            // Skip the sampler's own background thread to prevent deadlock.
            if skip_thread == Some(thread) {
                unsafe { mach_port_deallocate(mach_task_self(), thread) };
                continue;
            }

            let index = non_skip_index;
            non_skip_index += 1;

            let kr = unsafe { thread_suspend(thread) };
            if kr != KERN_SUCCESS {
                unsafe { mach_port_deallocate(mach_task_self(), thread) };
                continue;
            }

            let (thread_id, thread_name) = self.get_thread_identity(thread);

            if self.thread_matches_filter(thread, thread_id, &thread_name, index)
                && let Some(stack) = self.read_thread_stack(thread)
            {
                samples.push(ThreadSample {
                    thread_id,
                    thread_name,
                    stack,
                    timestamp_nanos: Instant::now().duration_since(start_time).as_nanos() as u64,
                });
            }

            unsafe { thread_resume(thread) };
            unsafe { mach_port_deallocate(mach_task_self(), thread) };
        }

        // Free the thread list.
        unsafe {
            mach_vm_deallocate(
                mach_task_self(),
                thread_list as mach_vm_address_t,
                (thread_count as usize * std::mem::size_of::<thread_act_t>()) as u64,
            );
        }

        samples
    }

    fn read_thread_stack(&self, thread: thread_act_t) -> Option<Vec<u64>> {
        let mut state = std::mem::MaybeUninit::<ThreadState>::uninit();
        let mut count: mach_msg_type_number_t = THREAD_STATE_COUNT;

        let kr = unsafe {
            thread_get_state(
                thread,
                THREAD_STATE_FLAVOR,
                state.as_mut_ptr() as *mut libc::c_void,
                &mut count,
            )
        };
        if kr != KERN_SUCCESS {
            return None;
        }

        let state = unsafe { state.assume_init() };

        #[cfg(target_arch = "aarch64")]
        let (pc, fp) = (strip_pac(state.pc), strip_pac(state.fp));

        #[cfg(target_arch = "x86_64")]
        let (pc, fp) = (strip_pac(state.rip), strip_pac(state.rbp));

        let mut addresses = vec![pc];
        self.walk_frame_pointers(fp, &mut addresses);
        Some(addresses)
    }

    /// Walk the frame pointer chain in the target process, appending return addresses.
    fn walk_frame_pointers(&self, start_fp: u64, addresses: &mut Vec<u64>) {
        const MAX_DEPTH: usize = 256;
        let mut fp = start_fp;

        for _ in 0..MAX_DEPTH {
            if fp == 0 || fp & 0x7 != 0 {
                break; // unaligned or null — end of chain
            }

            // Each frame: { previous_fp: u64, return_addr: u64 }
            let mut frame = [0u8; 16];
            if read_target_mem(self.task, fp, &mut frame).is_err() {
                break;
            }

            let prev_fp = u64::from_le_bytes(frame[0..8].try_into().unwrap());
            let ret_addr = u64::from_le_bytes(frame[8..16].try_into().unwrap());

            let ret_addr = strip_pac(ret_addr);
            if ret_addr == 0 {
                break;
            }
            addresses.push(ret_addr);

            if prev_fp <= fp {
                break; // stack must grow downward; guard against cycles
            }
            fp = prev_fp;
        }
    }

    /// Run the sampling loop until `stop` is set, the deadline elapses, or the
    /// target exits. Sets `stop` to `true` before returning so that callers
    /// polling [`Profile::is_stopped`] see an accurate value regardless of
    /// which exit condition triggered.
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

        // When profiling the current process, capture and skip the sampler's own
        // thread to prevent it from suspending itself (deadlock).
        let skip_thread = if self.is_self {
            Some(unsafe { mach_thread_self() })
        } else {
            None
        };

        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            if let Some(ref mut check) = check_child_exit
                && check()
            {
                break;
            }
            if let Some(d) = deadline
                && Instant::now() >= d
            {
                break;
            }

            if Instant::now() >= next_image_refresh {
                let refreshed = self.read_loaded_images().unwrap_or_default();
                let mut current = live.lock().unwrap();
                current.images =
                    prefer_loaded_images(std::mem::take(&mut current.images), refreshed);
                next_image_refresh = Instant::now() + Duration::from_millis(500);
            }

            let round = self.sample_once(skip_thread, start_time);
            {
                let mut current = live.lock().unwrap();
                for sample in round {
                    if !sample.stack.is_empty() {
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
                }
                current.end_time = Instant::now();
            }

            std::thread::sleep(interval);
        }

        // Always mark stopped so callers polling is_stopped() see the right value,
        // regardless of whether exit was due to a manual signal, child exit, or deadline.
        stop.store(true, Ordering::Relaxed);

        let end_time = Instant::now();

        // Release the sampler thread port we captured above.
        if let Some(thread) = skip_thread {
            unsafe { mach_port_deallocate(mach_task_self(), thread) };
        }

        let images = self.read_loaded_images().unwrap_or_default();
        let mut current = live.lock().unwrap();
        current.start_time = start_time;
        current.end_time = end_time;
        current.images = prefer_loaded_images(std::mem::take(&mut current.images), images);
        let result = current.clone();
        Ok(result)
    }

    /// Read the list of images loaded in the target process via dyld's all_image_infos.
    pub fn read_loaded_images(&self) -> Result<Vec<LoadedImage>> {
        let mut dyld_info = TaskDyldInfo {
            all_image_info_addr: 0,
            all_image_info_size: 0,
            all_image_info_format: 0,
            _pad: 0,
        };
        let mut count: mach_msg_type_number_t = TASK_DYLD_INFO_COUNT;

        let kr = unsafe {
            task_info(
                self.task,
                TASK_DYLD_INFO,
                &mut dyld_info as *mut TaskDyldInfo as *mut libc::c_int,
                &mut count,
            )
        };
        mach_check(kr, "task_info(TASK_DYLD_INFO)")?;

        let info_addr = dyld_info.all_image_info_addr;
        if info_addr == 0 {
            bail!("dyld all_image_info_addr is 0 — process may not be started yet");
        }

        // Read the dyld_all_image_infos header.
        let mut all_infos = DyldAllImageInfos {
            version: 0,
            info_array_count: 0,
            info_array: 0,
            notification: 0,
            process_detached_from_shared_region: 0,
            lib_system_initialized: 0,
            dyld_image_load_address: 0,
        };
        read_target_mem(self.task, info_addr, unsafe {
            std::slice::from_raw_parts_mut(
                &mut all_infos as *mut DyldAllImageInfos as *mut u8,
                std::mem::size_of::<DyldAllImageInfos>(),
            )
        })
        .context("reading dyld_all_image_infos")?;

        let count = all_infos.info_array_count as usize;
        let array_addr = all_infos.info_array;
        // Read the image info array.
        let entry_size = std::mem::size_of::<DyldImageInfo>();
        let mut images = Vec::with_capacity(count);

        for i in 0..count {
            if array_addr == 0 {
                break;
            }
            let addr = array_addr + (i * entry_size) as u64;
            let mut entry = DyldImageInfo {
                image_load_address: 0,
                image_file_path: 0,
                image_file_mod_date: 0,
            };
            if read_target_mem(self.task, addr, unsafe {
                std::slice::from_raw_parts_mut(
                    &mut entry as *mut DyldImageInfo as *mut u8,
                    entry_size,
                )
            })
            .is_err()
            {
                continue;
            }

            let path = if entry.image_file_path != 0 {
                read_target_cstr(self.task, entry.image_file_path).unwrap_or_default()
            } else {
                String::new()
            };

            if !path.is_empty() {
                images.push(LoadedImage {
                    load_address: entry.image_load_address,
                    path: resolve_image_path(&path, self.executable_path.as_deref()),
                });
            }
        }

        append_dyld_loader_image(
            &mut images,
            all_infos.version,
            all_infos.dyld_image_load_address,
        );

        Ok(images)
    }
}

fn process_executable_path(pid: u32) -> Option<String> {
    let mut path = [0u8; 4096];
    let length = unsafe {
        libc::proc_pidpath(
            pid as libc::c_int,
            path.as_mut_ptr().cast(),
            path.len() as u32,
        )
    };
    (length > 0).then(|| String::from_utf8_lossy(&path[..length as usize]).into_owned())
}

fn resolve_image_path(path: &str, executable_path: Option<&str>) -> String {
    let image_path = Path::new(path);
    if image_path.is_absolute() {
        return path.to_string();
    }
    if let Some(executable_path) = executable_path
        && image_path.file_name() == Path::new(executable_path).file_name()
    {
        return executable_path.to_string();
    }
    path.to_string()
}

fn append_dyld_loader_image(images: &mut Vec<LoadedImage>, version: u32, load_address: u64) {
    if version < 2
        || load_address == 0
        || images
            .iter()
            .any(|image| image.load_address == load_address)
    {
        return;
    }
    images.push(LoadedImage {
        load_address,
        path: "/usr/lib/dyld".to_string(),
    });
}

fn prefer_loaded_images(
    existing: Vec<LoadedImage>,
    refreshed: Vec<LoadedImage>,
) -> Vec<LoadedImage> {
    if refreshed.is_empty() {
        existing
    } else {
        refreshed
    }
}

impl Drop for MacosSampler {
    fn drop(&mut self) {
        if self.owns_task_port {
            unsafe { mach_port_deallocate(mach_task_self(), self.task) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_image_refresh_preserves_last_observed_images() {
        let existing = vec![LoadedImage {
            load_address: 0x1000,
            path: "/tmp/target".to_string(),
        }];

        let preserved = prefer_loaded_images(existing.clone(), Vec::new());
        assert_eq!(preserved.len(), 1);
        assert_eq!(preserved[0].path, "/tmp/target");

        let replacement = vec![LoadedImage {
            load_address: 0x2000,
            path: "/tmp/new-target".to_string(),
        }];
        let refreshed = prefer_loaded_images(existing, replacement);
        assert_eq!(refreshed.len(), 1);
        assert_eq!(refreshed[0].path, "/tmp/new-target");
    }

    #[test]
    fn test_dyld_loader_is_included_as_a_loaded_image() {
        assert_eq!(
            std::mem::offset_of!(DyldAllImageInfos, dyld_image_load_address),
            32
        );
        assert_eq!(std::mem::size_of::<DyldAllImageInfos>(), 40);
        let mut images = vec![LoadedImage {
            load_address: 0x1862_f3000,
            path: "/usr/lib/system/libdyld.dylib".to_string(),
        }];

        append_dyld_loader_image(&mut images, 2, 0x1863_28000);

        let dyld = images
            .iter()
            .find(|image| image.path == "/usr/lib/dyld")
            .expect("dyld loader image should be present");
        assert_eq!(dyld.load_address, 0x1863_28000);
    }

    #[test]
    fn relative_main_image_uses_process_executable_path() {
        let resolved = resolve_image_path(
            "target/debug/sequins-daemon",
            Some("/Users/test/sequins/target/debug/sequins-daemon"),
        );

        assert_eq!(resolved, "/Users/test/sequins/target/debug/sequins-daemon");
    }

    #[test]
    fn absolute_image_path_is_preserved() {
        let resolved = resolve_image_path(
            "/tmp/libexample.dylib",
            Some("/Users/test/sequins/target/debug/sequins-daemon"),
        );

        assert_eq!(resolved, "/tmp/libexample.dylib");
    }

    #[test]
    fn test_strip_pac_aarch64() {
        #[cfg(target_arch = "aarch64")]
        {
            let addr = 0xFFFF_8000_1234_5678u64;
            let stripped = strip_pac(addr);
            assert_eq!(stripped, 0x0000_0000_1234_5678u64);
        }
        #[cfg(target_arch = "x86_64")]
        {
            let addr = 0x0000_7FFF_1234_5678u64;
            assert_eq!(strip_pac(addr), addr);
        }
    }

    #[test]
    fn test_frame_pointer_alignment_check() {
        let misaligned_fp: u64 = 0x1001;
        assert_ne!(misaligned_fp & 0x7, 0);
    }
}
