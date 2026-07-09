use std::ffi::OsStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;

use crate::handle::ProfilerHandle;
use crate::sampler::{PlatformSampler, ThreadFilter};
use crate::symbolicate::Symbolizer;
use crate::symbolicated::{SymbolicatedProfile, Unresolved};

// Mach FFI needed for the closure profiler.
unsafe extern "C" {
    fn mach_thread_self() -> u32;
    fn mach_task_self() -> u32;
    fn mach_port_deallocate(task: u32, name: u32) -> i32;
}

/// Configure a profiling session, then select a target with a terminal method.
///
/// # Examples
///
/// ```no_run
/// # fn main() -> anyhow::Result<()> {
/// // Profile a child process
/// let handle = pprofessor::builder().freq(99).spawn("./my-binary", &["arg"])?;
///
/// // Attach to a running process for up to 5 seconds
/// let handle = pprofessor::builder()
///     .duration(std::time::Duration::from_secs(5))
///     .attach(12345)?;
///
/// // Profile the current process (no special permissions required)
/// let handle = pprofessor::builder().current()?;
///
/// // Profile only threads whose name contains "worker"
/// let handle = pprofessor::builder().thread_name("worker").attach(12345)?;
///
/// // Profile only the thread with the given ID
/// let handle = pprofessor::builder().thread_id(42).attach(12345)?;
///
/// // Profile the calling thread during a closure
/// let (result, profile) = pprofessor::builder().profile(|| 42u32)?;
/// let pprof_bytes = profile.to_pprof();
///
/// // Use a custom symbolizer (e.g. for JIT code)
/// use pprofessor::{FrameInfo, Symbolizer};
/// struct MySymbolizer;
/// impl Symbolizer for MySymbolizer {
///     fn symbolize_frame(&self, _addr: u64) -> Option<FrameInfo> { None }
/// }
/// let handle = pprofessor::builder().symbolizer(MySymbolizer).current()?;
/// # Ok(())
/// # }
/// ```
pub struct ProfilerBuilder {
    freq_hz: u32,
    thread_filter: ThreadFilter,
    duration: Option<Duration>,
    symbolizer: Option<Box<dyn Symbolizer>>,
    unresolved: Unresolved,
}

impl ProfilerBuilder {
    pub fn new() -> Self {
        Self {
            freq_hz: 99,
            thread_filter: ThreadFilter::All,
            duration: None,
            symbolizer: None,
            unresolved: Unresolved::default(),
        }
    }

    /// Set the sampling frequency in Hz (default: 99).
    pub fn freq(mut self, hz: u32) -> Self {
        self.freq_hz = hz;
        self
    }

    /// Stop sampling automatically after `d` has elapsed.
    ///
    /// Works in combination with other stop conditions — sampling stops at
    /// whichever comes first: the duration, a manual stop signal, or the
    /// target process exiting.
    pub fn duration(mut self, d: Duration) -> Self {
        self.duration = Some(d);
        self
    }

    /// Only sample threads whose name contains `name`.
    pub fn thread_name(mut self, name: impl Into<String>) -> Self {
        self.thread_filter = ThreadFilter::ByName(name.into());
        self
    }

    /// Only sample the main thread (index 0 in the OS thread list).
    pub fn main_thread(mut self) -> Self {
        self.thread_filter = ThreadFilter::MainThread;
        self
    }

    /// Only sample the thread with this numeric thread ID.
    pub fn thread_id(mut self, id: u64) -> Self {
        self.thread_filter = ThreadFilter::ById(id);
        self
    }

    /// Only sample the thread with this Mach thread port (internal use).
    fn thread_mach_port(mut self, port: u32) -> Self {
        self.thread_filter = ThreadFilter::ByMachThread(port);
        self
    }

    /// Use a custom symbolizer instead of (or in addition to) the default
    /// native DWARF symbolizer.
    ///
    /// The custom symbolizer is tried first. For addresses it returns `None`,
    /// the native DWARF symbolizer is used as a fallback. Compose multiple
    /// custom symbolizers with [`SymbolizerChain`](crate::SymbolizerChain).
    pub fn symbolizer(mut self, s: impl Symbolizer + 'static) -> Self {
        self.symbolizer = Some(Box::new(s));
        self
    }

    /// Configure behavior for addresses that no symbolizer can resolve.
    ///
    /// Default: [`Unresolved::Hex`] — unresolvable frames are kept with hex address names.
    pub fn unresolved(mut self, mode: Unresolved) -> Self {
        self.unresolved = mode;
        self
    }

    /// Spawn a child process and attach a profiler to it.
    ///
    /// Requires root or the `com.apple.security.cs.debugger` entitlement.
    pub fn spawn(
        self,
        binary: impl AsRef<OsStr>,
        args: &[impl AsRef<OsStr>],
    ) -> Result<ProfilerHandle> {
        let mut cmd = std::process::Command::new(binary.as_ref());
        for arg in args {
            cmd.arg(arg.as_ref());
        }
        let (child, mut sampler) = PlatformSampler::spawn(&mut cmd, self.freq_hz)?;
        sampler.thread_filter = self.thread_filter;
        Ok(ProfilerHandle::new_spawn(
            child,
            Arc::new(sampler),
            self.freq_hz,
            self.duration,
            self.symbolizer,
            self.unresolved,
        ))
    }

    /// Attach a profiler to an already-running process by PID.
    ///
    /// Requires root or the `com.apple.security.cs.debugger` entitlement.
    pub fn attach(self, pid: u32) -> Result<ProfilerHandle> {
        let mut sampler = PlatformSampler::new(pid, self.freq_hz)?;
        sampler.thread_filter = self.thread_filter;
        Ok(ProfilerHandle::new_attach(
            pid,
            Arc::new(sampler),
            self.freq_hz,
            self.duration,
            self.symbolizer,
            self.unresolved,
        ))
    }

    /// Profile the current process (no special permissions required).
    ///
    /// The sampler automatically skips its own thread to prevent deadlock.
    pub fn current(self) -> Result<ProfilerHandle> {
        let mut sampler = PlatformSampler::new_self(self.freq_hz)?;
        sampler.thread_filter = self.thread_filter;
        Ok(ProfilerHandle::new_current(
            Arc::new(sampler),
            self.freq_hz,
            self.duration,
            self.symbolizer,
            self.unresolved,
        ))
    }

    /// Profile only the calling thread while executing `f`.
    ///
    /// Samples are collected at the configured frequency. Returns the
    /// closure's return value together with a [`SymbolicatedProfile`].
    /// Call [`SymbolicatedProfile::to_pprof`] on the result to encode as
    /// pprof protobuf bytes.
    ///
    /// No special permissions are required (uses `mach_task_self`).
    pub fn profile<T>(self, f: impl FnOnce() -> T) -> Result<(T, SymbolicatedProfile)> {
        // Capture the Mach send-right for the calling thread so the sampler
        // can filter to exactly this thread.
        let calling_thread = unsafe { mach_thread_self() };

        let mut handle = self.thread_mach_port(calling_thread).current()?;
        let session = handle.start()?;

        let result = f();

        let data = session.stop()?;

        // Release the send-right we captured above.
        unsafe { mach_port_deallocate(mach_task_self(), calling_thread) };

        Ok((result, data))
    }
}

impl Default for ProfilerBuilder {
    fn default() -> Self {
        Self::new()
    }
}
