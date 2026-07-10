use std::collections::HashMap;
use std::time::Instant;

/// A loaded image (shared library or executable) in the target process.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    pub timestamp_nanos: u64,
}

/// Aggregated observations of one `(thread, stack)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSampleSeries {
    pub count: u64,
    pub timestamps_nanos: Option<Vec<u64>>,
}

impl RawSampleSeries {
    pub fn untimed(count: u64) -> Self {
        Self {
            count,
            timestamps_nanos: None,
        }
    }

    pub fn timed(timestamps_nanos: Vec<u64>) -> Self {
        Self {
            count: timestamps_nanos.len() as u64,
            timestamps_nanos: Some(timestamps_nanos),
        }
    }

    pub(crate) fn push_timestamp(&mut self, timestamp_nanos: u64) {
        self.count = self.count.saturating_add(1);
        self.timestamps_nanos
            .get_or_insert_with(Vec::new)
            .push(timestamp_nanos);
    }
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
#[derive(Clone)]
pub struct RawProfile {
    /// Maps `(thread_id, stack)` to counts and optional observation timestamps.
    pub stacks: HashMap<(u64, Vec<u64>), RawSampleSeries>,
    /// Maps thread ID to the last-observed thread name.
    pub thread_names: HashMap<u64, String>,
    pub start_time: Instant,
    pub end_time: Instant,
    /// Loaded images captured from the target at profile end (for symbolication).
    pub images: Vec<LoadedImage>,
}

#[derive(Clone, Default)]
pub struct RawProfileCursor {
    counts: HashMap<(u64, Vec<u64>), u64>,
}

impl RawProfileCursor {
    pub fn delta(&mut self, profile: &RawProfile) -> Option<RawProfile> {
        let mut stacks = HashMap::new();
        for (key, series) in &profile.stacks {
            let previous = self.counts.get(key).copied().unwrap_or(0);
            if series.count > previous {
                let timestamps_nanos = series
                    .timestamps_nanos
                    .as_ref()
                    .map(|timestamps| timestamps.iter().skip(previous as usize).copied().collect());
                stacks.insert(
                    key.clone(),
                    RawSampleSeries {
                        count: series.count - previous,
                        timestamps_nanos,
                    },
                );
                self.counts.insert(key.clone(), series.count);
            }
        }
        if stacks.is_empty() {
            return None;
        }
        Some(RawProfile {
            stacks,
            thread_names: profile.thread_names.clone(),
            start_time: profile.start_time,
            end_time: profile.end_time,
            images: profile.images.clone(),
        })
    }
}

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::MacosSampler as PlatformSampler;
