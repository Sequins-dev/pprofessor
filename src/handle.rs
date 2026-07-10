use std::process::{Child, ExitStatus};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;

use crate::process_exists;
use crate::profile::Profile;
use crate::sampler::{PlatformSampler, RawProfile};
use crate::symbolicate::Symbolizer;
use crate::symbolicated::Unresolved;

/// A handle to a profiling target. Created via [`crate::builder()`].
///
/// Call [`start()`](ProfilerHandle::start) to begin a sampling session.
/// The symbolizer is moved into the session on first `start()` — subsequent
/// sessions will use the default native symbolizer.
///
/// In Spawn mode, the child process is killed and reaped on drop.
pub struct ProfilerHandle {
    pub(crate) sampler: Arc<PlatformSampler>,
    pub(crate) freq_hz: u32,
    pub(crate) duration: Option<Duration>,
    pub(crate) symbolizer: Option<Box<dyn Symbolizer>>,
    pub(crate) unresolved: Unresolved,
    pub(crate) inner: HandleInner,
}

pub(crate) enum HandleInner {
    Spawn { child: Arc<Mutex<Child>> },
    Attach { pid: u32 },
    Current,
}

impl ProfilerHandle {
    pub(crate) fn new_spawn(
        child: Child,
        sampler: Arc<PlatformSampler>,
        freq_hz: u32,
        duration: Option<Duration>,
        symbolizer: Option<Box<dyn Symbolizer>>,
        unresolved: Unresolved,
    ) -> Self {
        ProfilerHandle {
            sampler,
            freq_hz,
            duration,
            symbolizer,
            unresolved,
            inner: HandleInner::Spawn {
                child: Arc::new(Mutex::new(child)),
            },
        }
    }

    pub(crate) fn new_attach(
        pid: u32,
        sampler: Arc<PlatformSampler>,
        freq_hz: u32,
        duration: Option<Duration>,
        symbolizer: Option<Box<dyn Symbolizer>>,
        unresolved: Unresolved,
    ) -> Self {
        ProfilerHandle {
            sampler,
            freq_hz,
            duration,
            symbolizer,
            unresolved,
            inner: HandleInner::Attach { pid },
        }
    }

    pub(crate) fn new_current(
        sampler: Arc<PlatformSampler>,
        freq_hz: u32,
        duration: Option<Duration>,
        symbolizer: Option<Box<dyn Symbolizer>>,
        unresolved: Unresolved,
    ) -> Self {
        ProfilerHandle {
            sampler,
            freq_hz,
            duration,
            symbolizer,
            unresolved,
            inner: HandleInner::Current,
        }
    }

    /// Returns the PID of the profiling target.
    pub fn pid(&self) -> u32 {
        match &self.inner {
            HandleInner::Spawn { child } => child.lock().unwrap().id(),
            HandleInner::Attach { pid } => *pid,
            HandleInner::Current => std::process::id(),
        }
    }

    /// Wait for the child process to exit (Spawn mode only).
    ///
    /// Returns `None` in Attach and Current modes.
    pub fn wait(&mut self) -> Option<Result<ExitStatus>> {
        match &mut self.inner {
            HandleInner::Spawn { child } => Some(child.lock().unwrap().wait().map_err(Into::into)),
            _ => None,
        }
    }

    /// Start a new sampling session.
    ///
    /// Spawns a background thread that samples the target at the configured
    /// frequency. Call [`Profile::stop`] to end the session and retrieve a
    /// [`crate::SymbolicatedProfile`].
    ///
    /// The custom symbolizer (if any) is moved into this session. Subsequent
    /// calls to `start()` will use only the native symbolizer.
    pub fn start(&mut self) -> Result<Profile> {
        let stop = Arc::new(AtomicBool::new(false));
        let sampler = Arc::clone(&self.sampler);
        let freq_hz = self.freq_hz;
        let start_wall = SystemTime::now();
        let deadline = self.duration.map(|d| Instant::now() + d);
        let live = Arc::new(Mutex::new(RawProfile {
            stacks: Default::default(),
            thread_names: Default::default(),
            start_time: Instant::now(),
            end_time: Instant::now(),
            images: self.sampler.read_loaded_images().unwrap_or_default(),
        }));

        let check: Option<Box<dyn FnMut() -> bool + Send>> = match &self.inner {
            HandleInner::Spawn { child } => {
                let child = Arc::clone(child);
                Some(Box::new(move || {
                    child.lock().unwrap().try_wait().ok().flatten().is_some()
                }))
            }
            HandleInner::Attach { pid } => {
                let pid = *pid;
                Some(Box::new(move || !process_exists(pid)))
            }
            HandleInner::Current => None,
        };

        let stop_clone = Arc::clone(&stop);
        let live_clone = Arc::clone(&live);
        let thread = std::thread::spawn(move || {
            sampler.run_sampling_loop(stop_clone, check, deadline, live_clone)
        });

        let symbolizer = self.symbolizer.take();
        let unresolved = self.unresolved;

        Ok(Profile::new(
            stop, thread, live, freq_hz, start_wall, symbolizer, unresolved,
        ))
    }
}

impl Drop for ProfilerHandle {
    fn drop(&mut self) {
        if let HandleInner::Spawn { child } = &self.inner {
            let mut child = child.lock().unwrap();
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
