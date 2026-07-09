use std::collections::HashMap;
use std::time::Instant;

/// A loaded image (shared library or executable) in the target process.
#[derive(Debug, Clone)]
pub struct LoadedImage {
    /// Address at which the image's __TEXT segment was loaded.
    pub load_address: u64,
    /// Path to the binary on disk.
    pub path: String,
}

/// A single stack sample with the thread identity that produced it.
pub struct ThreadSample {
    pub thread_id: u64,
    pub thread_name: String,
    pub stack: Vec<u64>,
}

/// Controls which threads are sampled.
#[derive(Clone, Default)]
pub enum ThreadFilter {
    /// Sample all threads (default).
    #[default]
    All,
    /// Sample only the main thread (index 0 in the OS thread list).
    MainThread,
    /// Sample only threads whose name contains this substring.
    ByName(String),
    /// Sample only the thread with this numeric thread ID.
    ById(u64),
    /// Sample only the thread with this Mach thread port.
    /// Used internally by the closure profiler to pin sampling to one thread.
    ByMachThread(u32),
}

/// Raw sample data accumulated across the profiling session.
pub struct RawProfile {
    /// Maps `(thread_id, stack)` to sample counts.
    pub stacks: HashMap<(u64, Vec<u64>), u64>,
    /// Maps thread ID to the last-observed thread name.
    pub thread_names: HashMap<u64, String>,
    pub start_time: Instant,
    pub end_time: Instant,
    /// Loaded images captured from the target at profile end (for symbolication).
    pub images: Vec<LoadedImage>,
}

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::MacosSampler as PlatformSampler;
