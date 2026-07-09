use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::SystemTime;

use anyhow::Result;

use crate::sampler::RawProfile;
use crate::symbolicate::{FrameInfo, NativeSymbolizer, Symbolizer, SymbolizerChain};
use crate::symbolicated::{SymbolicatedProfile, Unresolved};

/// An active sampling session. Created by [`crate::ProfilerHandle::start`].
///
/// Call [`stop`](Profile::stop) to end the session and retrieve a
/// [`SymbolicatedProfile`]. If dropped before `stop()` is called, the
/// background thread is stopped and joined automatically.
pub struct Profile {
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) thread: Option<JoinHandle<Result<RawProfile>>>,
    freq_hz: u32,
    start_wall: SystemTime,
    symbolizer: Option<Box<dyn Symbolizer>>,
    unresolved: Unresolved,
}

impl Profile {
    pub(crate) fn new(
        stop: Arc<AtomicBool>,
        thread: JoinHandle<Result<RawProfile>>,
        freq_hz: u32,
        start_wall: SystemTime,
        symbolizer: Option<Box<dyn Symbolizer>>,
        unresolved: Unresolved,
    ) -> Self {
        Profile {
            stop,
            thread: Some(thread),
            freq_hz,
            start_wall,
            symbolizer,
            unresolved,
        }
    }

    /// Signal the background sampler to stop. Non-blocking.
    pub fn signal_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Returns true once the background sampler has stopped (either because
    /// [`signal_stop`](Profile::signal_stop) was called, a deadline elapsed,
    /// or the profiling target exited).
    pub fn is_stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// Stop sampling, symbolicate the collected stacks, and return a
    /// [`SymbolicatedProfile`].
    ///
    /// Call [`SymbolicatedProfile::to_pprof`] to encode as pprof protobuf bytes.
    pub fn stop(mut self) -> Result<SymbolicatedProfile> {
        self.signal_stop();
        let thread = self.thread.take().expect("thread already joined");
        let raw = thread
            .join()
            .map_err(|_| anyhow::anyhow!("sampling thread panicked"))??;

        // Collect unique addresses for NativeSymbolizer construction.
        let unique_addrs: Vec<u64> = {
            let mut addrs: HashSet<u64> = HashSet::new();
            for (_tid, stack) in raw.stacks.keys() {
                addrs.extend(stack.iter());
            }
            addrs.into_iter().collect()
        };

        // Build the effective symbolizer: custom (if any) chained with native fallback.
        let native = NativeSymbolizer::new(raw.images.clone(), &unique_addrs);
        let symbolizer: Box<dyn Symbolizer> = match self.symbolizer.take() {
            Some(custom) => Box::new(SymbolizerChain::new(vec![custom, Box::new(native)])),
            None => Box::new(native),
        };

        Ok(SymbolicatedProfile::from_raw_with_symbolizer(
            raw,
            &*symbolizer,
            self.unresolved,
            self.freq_hz,
            self.start_wall,
        ))
    }

    /// Stop sampling and return a [`SymbolicatedProfile`] without DWARF symbolication.
    ///
    /// All frames use hex address strings (e.g. `"0x00007fff12345678"`).
    /// Faster than [`stop`](Profile::stop) when symbol names are not needed.
    pub fn stop_unsymbolicated(mut self) -> Result<SymbolicatedProfile> {
        self.signal_stop();
        let thread = self.thread.take().expect("thread already joined");
        let raw = thread
            .join()
            .map_err(|_| anyhow::anyhow!("sampling thread panicked"))??;

        // Use a no-op symbolizer — all addresses fall through to Unresolved::Hex.
        struct HexSymbolizer;
        impl Symbolizer for HexSymbolizer {
            fn symbolize_frame(&self, _address: u64) -> Option<FrameInfo> {
                None
            }
        }

        Ok(SymbolicatedProfile::from_raw_with_symbolizer(
            raw,
            &HexSymbolizer,
            Unresolved::Hex,
            self.freq_hz,
            self.start_wall,
        ))
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        // Ensure the background thread is always stopped and joined, even if
        // the caller drops the Profile without calling stop().
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
