//! Heap measurements represented entirely as standard pprof profiles.
//!
//! Native collectors and runtime integrations share stacks, values and export.
//! A heap domain is a separate profile: backing storage and suballocations must
//! never be summed together as if they were disjoint memory.
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::pprof::{Function, Label, Line, Location, Mapping, PprofProfile, Sample, ValueType};
use anyhow::{Result, bail, ensure};

#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeapFrame {
    pub address: u64,
    pub function: String,
    pub file: String,
    pub line: i64,
    pub mapping: Option<HeapMapping>,
}

/// Native image identity and address range, preserved in pprof mappings.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HeapMapping {
    pub memory_start: u64,
    pub memory_limit: u64,
    pub file_offset: u64,
    pub filename: String,
    pub build_id: String,
}

/// Exact counts, or estimates already corrected by the sampling provider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeapValues {
    pub alloc_objects: u64,
    pub alloc_space: u64,
    pub inuse_objects: u64,
    pub inuse_space: u64,
}
impl HeapValues {
    fn values(self) -> [u64; 4] {
        [
            self.alloc_objects,
            self.alloc_space,
            self.inuse_objects,
            self.inuse_space,
        ]
    }
    fn checked_add(self, other: Self) -> Result<Self> {
        let a = self.values();
        let b = other.values();
        let mut result = [0; 4];
        for i in 0..4 {
            result[i] = a[i]
                .checked_add(b[i])
                .filter(|v| *v <= i64::MAX as u64)
                .ok_or_else(|| anyhow::anyhow!("heap measurement exceeds pprof int64 range"))?;
        }
        Ok(Self {
            alloc_objects: result[0],
            alloc_space: result[1],
            inuse_objects: result[2],
            inuse_space: result[3],
        })
    }
}

/// One heap checkpoint. Runtime integrations can supply already aggregated
/// allocation stacks without emulating native addresses or object lifecycles.
#[derive(Clone, Debug)]
pub struct HeapProfile {
    pub domain: String,
    pub collected_at: SystemTime,
    pub comment: String,
    samples: BTreeMap<Vec<HeapFrame>, HeapValues>,
}
impl HeapProfile {
    pub fn new(domain: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            collected_at: SystemTime::now(),
            comment: String::new(),
            samples: BTreeMap::new(),
        }
    }
    /// Frames are leaf first, matching pprof's location order.
    pub fn add_sample(&mut self, stack: Vec<HeapFrame>, values: HeapValues) -> Result<()> {
        ensure!(
            !stack.is_empty(),
            "heap samples require an allocation stack"
        );
        let previous = self.samples.get(&stack).copied().unwrap_or_default();
        self.samples.insert(stack, previous.checked_add(values)?);
        Ok(())
    }
    pub fn to_pprof(&self) -> Result<PprofProfile> {
        ensure!(!self.domain.is_empty(), "heap domain must not be empty");
        let mut p = PprofProfile::new();
        for (name, unit) in [
            ("alloc_objects", "count"),
            ("alloc_space", "bytes"),
            ("inuse_objects", "count"),
            ("inuse_space", "bytes"),
        ] {
            p.value_types.push(ValueType {
                r#type: p.strings.intern(name),
                unit: p.strings.intern(unit),
            });
        }
        p.default_sample_type = p.strings.intern("inuse_space");
        p.time_nanos = self
            .collected_at
            .duration_since(UNIX_EPOCH)?
            .as_nanos()
            .try_into()?;
        let label = Label {
            key: p.strings.intern("heap"),
            str_index: p.strings.intern(&self.domain),
            ..Default::default()
        };
        if !self.comment.is_empty() {
            p.comments.push(p.strings.intern(&self.comment));
        }
        let mut frames = BTreeMap::new();
        // Keep native images in address order, with the executable ahead of
        // its loaded libraries, rather than making the leaf allocator image
        // the profile's apparent main binary.
        let mut image_flags = BTreeMap::<&HeapMapping, (bool, bool, bool)>::new();
        for frame in self.samples.keys().flatten() {
            if let Some(image) = frame.mapping.as_ref() {
                let flags = image_flags.entry(image).or_default();
                flags.0 |= !frame.function.is_empty();
                flags.1 |= !frame.file.is_empty();
                flags.2 |= frame.line != 0;
            }
        }
        let mut mappings = BTreeMap::new();
        for (image, flags) in image_flags {
            let id = p.mappings.len() as u64 + 1;
            mappings.insert(image, id);
            p.mappings.push(Mapping {
                id,
                memory_start: image.memory_start,
                memory_limit: image.memory_limit,
                file_offset: image.file_offset,
                filename: p.strings.intern(&image.filename),
                build_id: p.strings.intern(&image.build_id),
                has_functions: flags.0,
                has_filenames: flags.1,
                has_line_numbers: flags.2,
                ..Default::default()
            });
        }
        for (stack, values) in &self.samples {
            let mut ids = Vec::new();
            for frame in stack {
                let id = *frames.entry(frame).or_insert_with(|| {
                    let id = p.functions.len() as u64 + 1;
                    let name = if frame.function.is_empty() {
                        format!("0x{:016x}", frame.address)
                    } else {
                        frame.function.clone()
                    };
                    let name = p.strings.intern(&name);
                    p.functions.push(Function {
                        id,
                        name,
                        system_name: name,
                        filename: p.strings.intern(&frame.file),
                        ..Default::default()
                    });
                    let mapping_id = frame
                        .mapping
                        .as_ref()
                        .and_then(|image| mappings.get(image))
                        .copied()
                        .unwrap_or(0);
                    p.locations.push(Location {
                        id,
                        mapping_id,
                        address: frame.address,
                        lines: vec![Line {
                            function_id: id,
                            line: frame.line,
                            ..Default::default()
                        }],
                        ..Default::default()
                    });
                    id
                });
                ids.push(id);
            }
            p.samples.push(Sample {
                location_ids: ids,
                values: values.values().into_iter().map(|v| v as i64).collect(),
                labels: vec![label.clone()],
            });
        }
        if p.samples.is_empty() {
            p.samples.push(Sample {
                values: vec![0; 4],
                labels: vec![label],
                ..Default::default()
            });
        }
        Ok(p)
    }
}

/// Strict allocation accounting for ordered, complete native allocation logs.
/// Unknown frees indicate missing history and fail instead of undercounting.
pub struct NativeHeapTracker {
    domain: String,
    stack_ids: HashMap<Vec<HeapFrame>, usize>,
    stacks: Vec<(Vec<HeapFrame>, HeapValues)>,
    live: HashMap<u64, (u64, usize)>,
}
impl NativeHeapTracker {
    pub fn new(domain: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
            stack_ids: HashMap::new(),
            stacks: Vec::new(),
            live: HashMap::new(),
        }
    }
    pub fn allocate(&mut self, address: u64, size: u64, stack: Vec<HeapFrame>) -> Result<()> {
        ensure!(address != 0, "null is not a successful allocation");
        ensure!(
            !stack.is_empty(),
            "heap samples require an allocation stack"
        );
        ensure!(
            !self.live.contains_key(&address),
            "duplicate live allocation at {address:#x}"
        );
        let cost = HeapValues {
            alloc_objects: 1,
            alloc_space: size,
            ..Default::default()
        };
        let id = if let Some(id) = self.stack_ids.get(&stack).copied() {
            self.stacks[id].1 = self.stacks[id].1.checked_add(cost)?;
            id
        } else {
            let cost = HeapValues::default().checked_add(cost)?;
            let id = self.stacks.len();
            self.stack_ids.insert(stack.clone(), id);
            self.stacks.push((stack, cost));
            id
        };
        // A live allocation stores a small stack ID, not a duplicate backtrace.
        self.live.insert(address, (size, id));
        Ok(())
    }
    pub fn release(&mut self, address: u64) -> Result<()> {
        if address == 0 {
            return Ok(());
        }
        if self.live.remove(&address).is_none() {
            bail!("free without allocation history at {address:#x}");
        }
        Ok(())
    }
    pub fn snapshot(&self) -> Result<HeapProfile> {
        let mut values = self.stacks.iter().map(|(_, v)| *v).collect::<Vec<_>>();
        for (size, id) in self.live.values() {
            values[*id] = values[*id].checked_add(HeapValues {
                inuse_objects: 1,
                inuse_space: *size,
                ..Default::default()
            })?;
        }
        let mut profile = HeapProfile::new(&self.domain);
        for ((stack, _), values) in self.stacks.iter().zip(values) {
            profile.add_sample(stack.clone(), values)?;
        }
        Ok(profile)
    }
}

pub mod capture;
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

/// Validate the heap metric shape before a runtime profile enters a session.
pub fn validate_profile(profile: &PprofProfile) -> Result<()> {
    ensure!(
        profile
            .strings
            .strings
            .first()
            .is_some_and(|s| s.is_empty()),
        "pprof string table must start with an empty string"
    );
    let valid_string = |id: u64| -> Result<()> {
        ensure!(
            (id as u128) < profile.strings.strings.len() as u128,
            "invalid pprof string index {id}"
        );
        Ok(())
    };
    let unique_ids = |ids: Vec<u64>| -> Result<std::collections::HashSet<u64>> {
        let set = ids
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        ensure!(
            set.len() == ids.len() && !set.contains(&0),
            "duplicate or zero pprof identifier"
        );
        Ok(set)
    };
    let mappings = unique_ids(profile.mappings.iter().map(|m| m.id).collect())?;
    let functions = unique_ids(profile.functions.iter().map(|f| f.id).collect())?;
    let locations = unique_ids(profile.locations.iter().map(|l| l.id).collect())?;
    for mapping in &profile.mappings {
        valid_string(mapping.filename)?;
        valid_string(mapping.build_id)?;
    }
    for function in &profile.functions {
        valid_string(function.name)?;
        valid_string(function.system_name)?;
        valid_string(function.filename)?;
    }
    for location in &profile.locations {
        ensure!(
            location.mapping_id == 0 || mappings.contains(&location.mapping_id),
            "missing pprof mapping"
        );
        for line in &location.lines {
            ensure!(
                functions.contains(&line.function_id),
                "missing pprof function"
            );
        }
    }
    for sample in &profile.samples {
        for id in &sample.location_ids {
            ensure!(locations.contains(id), "missing pprof location");
        }
        for label in &sample.labels {
            valid_string(label.key)?;
            valid_string(label.str_index)?;
            valid_string(label.num_unit)?;
        }
    }
    for id in profile.comments.iter().copied().chain([
        profile.default_sample_type,
        profile.doc_url,
        profile.drop_frames,
        profile.keep_frames,
        profile.period_type.r#type,
        profile.period_type.unit,
    ]) {
        valid_string(id)?;
    }
    ensure!(
        !profile.value_types.is_empty(),
        "heap profile has no sample types"
    );
    let string = |id: u64| {
        profile
            .strings
            .strings
            .get(id as usize)
            .map(String::as_str)
            .unwrap_or("")
    };
    let mut seen = std::collections::HashSet::new();
    for metric in &profile.value_types {
        valid_string(metric.r#type)?;
        valid_string(metric.unit)?;
        let name = string(metric.r#type);
        ensure!(seen.insert(name), "duplicate heap metric");
        let unit = string(metric.unit);
        ensure!(
            matches!(
                (name, unit),
                ("alloc_space" | "inuse_space", "bytes")
                    | ("alloc_objects" | "inuse_objects", "count")
            ),
            "unsupported heap metric {name}/{unit}"
        );
    }
    for sample in &profile.samples {
        ensure!(
            sample.values.len() == profile.value_types.len(),
            "heap sample metric count does not match sample types"
        );
    }
    Ok(())
}
pub mod compare;

/// Select one pprof-labelled heap domain before computing totals. Unlabelled
/// heap samples are a distinct domain, never implicitly added to labelled ones.
pub fn select_domain(profile: &mut PprofProfile, requested: Option<&str>) -> Result<()> {
    let heap = profile.value_types.iter().any(|v| {
        profile
            .strings
            .strings
            .get(v.r#type as usize)
            .is_some_and(|s| {
                matches!(
                    s.as_str(),
                    "alloc_space" | "inuse_space" | "alloc_objects" | "inuse_objects"
                )
            })
    });
    if !heap {
        ensure!(
            requested.is_none(),
            "heap domain selection requires a heap profile"
        );
        return Ok(());
    }
    let domain = |sample: &Sample| {
        sample
            .labels
            .iter()
            .find(|l| {
                profile
                    .strings
                    .strings
                    .get(l.key as usize)
                    .is_some_and(|s| s == "heap")
            })
            .and_then(|l| profile.strings.strings.get(l.str_index as usize))
            .map(String::as_str)
            .unwrap_or("[unspecified heap]")
    };
    let domains = profile
        .samples
        .iter()
        .map(&domain)
        .collect::<std::collections::BTreeSet<_>>();
    ensure!(
        domains.len() <= 1 || requested.is_some(),
        "profile contains overlapping heap domains; select one with --heap: {}",
        domains.iter().copied().collect::<Vec<_>>().join(", ")
    );
    if let Some(requested) = requested {
        ensure!(
            domains.contains(requested),
            "unknown heap domain {requested}"
        );
        let samples = profile
            .samples
            .iter()
            .filter(|s| domain(s) == requested)
            .cloned()
            .collect();
        profile.samples = samples;
    }
    Ok(())
}
