//! Structured, symbolicated profile data.
//!
//! [`SymbolicatedProfile`] is the central output type of a profiling session.
//! It holds all resolved samples in a human-readable form and can be serialized
//! to various output formats. Call [`SymbolicatedProfile::to_pprof`] to produce
//! a gzip-ready pprof protobuf, or inspect the fields directly for custom
//! analysis.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::encode;
use crate::sampler::RawProfile;
use crate::symbolicate::Symbolizer;

// ---------------------------------------------------------------------------
// Unresolved frame behavior
// ---------------------------------------------------------------------------

/// What to do when no symbolizer can resolve an address.
#[derive(Debug, Clone, Copy, Default)]
pub enum Unresolved {
    /// Keep the frame with a hex address string (e.g. `"0x00007fff12345678"`).
    /// This is the default — no samples are silently dropped.
    #[default]
    Hex,
    /// Skip the frame: remove it from the output, flattening its children to
    /// the parent frame. Useful when you want cleaner profiles that omit
    /// frames from system libraries that cannot be symbolicated.
    Skip,
}

// ---------------------------------------------------------------------------
// Profile data model
// ---------------------------------------------------------------------------

/// A fully symbolicated CPU profile ready for inspection or serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolicatedProfile {
    /// Frame pool keyed by address. Each unique address appears at most once.
    /// Addresses absent from this map were skipped by the symbolizer.
    #[serde(with = "frame_map_serde")]
    pub frames: HashMap<u64, StackFrame>,
    /// Thread metadata: thread_id → thread_name.
    pub threads: HashMap<u64, String>,
    /// All collected samples, one entry per unique (thread, stack) pair.
    pub samples: Vec<Sample>,
    /// Wall-clock time at which sampling started.
    #[serde(with = "system_time_serde")]
    pub start_time: SystemTime,
    /// Total elapsed time of the profiling session.
    #[serde(with = "duration_serde")]
    pub duration: Duration,
    /// Sampling frequency in Hz.
    pub freq_hz: u32,
}

/// A single aggregated sample: a thread + call stack observed `count` times.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// Stable numeric thread identifier (from the OS).
    pub thread_id: u64,
    /// Raw frame pointer addresses, leaf-first.
    /// Look up each address in [`SymbolicatedProfile::frames`] for symbolication
    /// data. Addresses absent from the frame pool were skipped by the symbolizer.
    pub stack: Vec<u64>,
    /// Number of times this exact stack was observed on this thread.
    pub count: u64,
}

/// A single frame in a call stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrame {
    /// Raw instruction pointer address.
    pub address: u64,
    /// Demangled function name, or `"0x{address:016x}"` if unresolved + hex mode.
    pub function: String,
    /// Source file path, or empty string if unavailable.
    pub file: String,
    /// Source line number, or `0` if unavailable.
    pub line: u32,
}

// ---------------------------------------------------------------------------
// Profile construction
// ---------------------------------------------------------------------------

impl SymbolicatedProfile {
    /// Build a `SymbolicatedProfile` by symbolizing `raw` using `symbolizer`.
    ///
    /// Each unique address is symbolized exactly once. The `unresolved` parameter
    /// controls what happens when no symbolizer can resolve an address:
    /// - [`Unresolved::Skip`]: frame is removed from output (default)
    /// - [`Unresolved::Hex`]: frame is kept with a hex address function name
    pub fn from_raw_with_symbolizer(
        raw: RawProfile,
        symbolizer: &dyn Symbolizer,
        unresolved: Unresolved,
        freq_hz: u32,
        start_wall: SystemTime,
    ) -> Self {
        let duration = raw.end_time.duration_since(raw.start_time);
        let threads: HashMap<u64, String> = raw.thread_names.clone();

        // Collect all unique addresses across all stacks.
        let all_addrs: HashSet<u64> = raw
            .stacks
            .keys()
            .flat_map(|(_, stack)| stack.iter().copied())
            .collect();

        // Symbolize each unique address exactly once.
        let mut frames: HashMap<u64, StackFrame> = HashMap::new();
        let mut skip_set: HashSet<u64> = HashSet::new();

        for &addr in &all_addrs {
            match symbolizer.symbolize_frame(addr) {
                Some(info) => {
                    frames.insert(
                        addr,
                        StackFrame {
                            address: addr,
                            function: info.function,
                            file: info.file,
                            line: info.line,
                        },
                    );
                }
                None => match unresolved {
                    Unresolved::Skip => {
                        skip_set.insert(addr);
                    }
                    Unresolved::Hex => {
                        frames.insert(
                            addr,
                            StackFrame {
                                address: addr,
                                function: format!("0x{addr:016x}"),
                                file: String::new(),
                                line: 0,
                            },
                        );
                    }
                },
            }
        }

        // Build samples, filtering out skipped addresses.
        let samples: Vec<Sample> = raw
            .stacks
            .into_iter()
            .filter_map(|((thread_id, stack_addrs), count)| {
                let stack: Vec<u64> = stack_addrs
                    .into_iter()
                    .filter(|addr| !skip_set.contains(addr))
                    .collect();
                if stack.is_empty() {
                    return None;
                }
                Some(Sample {
                    thread_id,
                    stack,
                    count,
                })
            })
            .collect();

        SymbolicatedProfile {
            frames,
            threads,
            samples,
            start_time: start_wall,
            duration,
            freq_hz,
        }
    }

    /// Encode this profile as a pprof protobuf.
    ///
    /// The returned bytes are **not** gzip-compressed. To write a `.pb.gz`
    /// file, wrap the result with a gzip encoder.
    pub fn to_pprof(&self) -> Bytes {
        encode::build_proto(self)
    }

    /// Resolve a sample's stack addresses to [`StackFrame`] references.
    ///
    /// Addresses not present in the frame pool (because they were skipped)
    /// are omitted from the result.
    pub fn resolve_stack<'a>(&'a self, sample: &'a Sample) -> Vec<&'a StackFrame> {
        sample
            .stack
            .iter()
            .filter_map(|addr| self.frames.get(addr))
            .collect()
    }

    /// Get root-level tree nodes for this profile. Children are computed lazily
    /// when [`TreeNode::children`] is called.
    ///
    /// This provides a tree view over the flat sample data, suitable for
    /// building flame graphs or other hierarchical visualizations.
    pub fn roots(&self) -> Vec<TreeNode<'_>> {
        let mut root_addrs: Vec<u64> = Vec::new();
        for sample in &self.samples {
            if let Some(&first) = sample.stack.last() {
                // stack is leaf-first, so last = root
                if !root_addrs.contains(&first) {
                    root_addrs.push(first);
                }
            }
        }
        root_addrs
            .into_iter()
            .map(|addr| TreeNode {
                profile: self,
                address: addr,
                prefix: vec![addr],
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Lazy tree navigation
// ---------------------------------------------------------------------------

/// A lazy tree node backed by flat [`SymbolicatedProfile`] data.
///
/// The tree is not materialized in memory — each [`TreeNode::children`] call
/// scans the sample list to find the next level. This is correct and
/// allocation-efficient for typical profile sizes.
pub struct TreeNode<'a> {
    profile: &'a SymbolicatedProfile,
    /// The address this node represents.
    pub address: u64,
    /// Root-first stack prefix from the root to this node (inclusive).
    prefix: Vec<u64>,
}

impl<'a> TreeNode<'a> {
    /// The resolved stack frame for this node, if available.
    pub fn frame(&self) -> Option<&'a StackFrame> {
        self.profile.frames.get(&self.address)
    }

    /// Number of samples where this frame is the deepest (leaf) frame.
    pub fn self_count(&self) -> u64 {
        self.profile
            .samples
            .iter()
            .filter(|s| {
                let rev: Vec<u64> = s.stack.iter().rev().copied().collect();
                rev.len() == self.prefix.len() && rev == self.prefix
            })
            .map(|s| s.count)
            .sum()
    }

    /// Total count: self_count plus all descendants.
    pub fn total_count(&self) -> u64 {
        self.profile
            .samples
            .iter()
            .filter(|s| {
                let rev: Vec<u64> = s.stack.iter().rev().copied().collect();
                rev.starts_with(&self.prefix)
            })
            .map(|s| s.count)
            .sum()
    }

    /// Lazily compute children: distinct next-level addresses from samples
    /// whose reversed stack starts with this node's prefix.
    pub fn children(&self) -> Vec<TreeNode<'a>> {
        let mut child_addrs: Vec<u64> = Vec::new();
        for s in &self.profile.samples {
            let rev: Vec<u64> = s.stack.iter().rev().copied().collect();
            if rev.len() > self.prefix.len() && rev.starts_with(&self.prefix) {
                let next_addr = rev[self.prefix.len()];
                if !child_addrs.contains(&next_addr) {
                    child_addrs.push(next_addr);
                }
            }
        }
        child_addrs
            .into_iter()
            .map(|addr| {
                let mut prefix = self.prefix.clone();
                prefix.push(addr);
                TreeNode {
                    profile: self.profile,
                    address: addr,
                    prefix,
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Serde helpers for SystemTime and Duration (not natively supported by serde)
// ---------------------------------------------------------------------------

mod system_time_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        t.duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_nanos()
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let nanos = u128::deserialize(d)?;
        Ok(UNIX_EPOCH + Duration::from_nanos(nanos as u64))
    }
}

mod duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_nanos().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let nanos = u128::deserialize(d)?;
        Ok(Duration::from_nanos(nanos as u64))
    }
}

// HashMap<u64, StackFrame> with string keys for JSON compat
mod frame_map_serde {
    use super::StackFrame;
    use std::collections::HashMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        map: &HashMap<u64, StackFrame>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        // Serialize as a list of (address, frame) pairs for portability.
        let pairs: Vec<(&u64, &StackFrame)> = map.iter().collect();
        pairs.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<HashMap<u64, StackFrame>, D::Error> {
        let pairs: Vec<(u64, StackFrame)> = Vec::deserialize(d)?;
        Ok(pairs.into_iter().collect())
    }
}
