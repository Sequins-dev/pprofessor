//! Signed differences of compatible heap pprof snapshots.
use crate::PprofProfile;
use crate::pprof::{Function, Location, Mapping, Sample};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Return current minus base. Symbolic frame identity ignores relocated PCs;
/// unsymbolicated locations use build identity and mapping-relative addresses.
/// Samples retain their labels, and different heap domains cannot be combined.
pub fn difference(base: &PprofProfile, current: &PprofProfile) -> Result<PprofProfile> {
    super::validate_profile(base)?;
    super::validate_profile(current)?;
    let string = |p: &PprofProfile, id: u64| {
        p.strings
            .strings
            .get(id as usize)
            .cloned()
            .unwrap_or_default()
    };
    let types = |p: &PprofProfile| {
        p.value_types
            .iter()
            .map(|v| (string(p, v.r#type), string(p, v.unit)))
            .collect::<Vec<_>>()
    };
    let a = types(base);
    let b = types(current);
    ensure!(
        a.len() == b.len() && a.iter().collect::<BTreeSet<_>>() == b.iter().collect(),
        "heap profiles have incompatible sample types"
    );
    let domains = |p: &PprofProfile| {
        p.samples
            .iter()
            .flat_map(|s| {
                s.labels
                    .iter()
                    .filter(|l| string(p, l.key) == "heap")
                    .map(|l| string(p, l.str_index))
            })
            .collect::<BTreeSet<_>>()
    };
    ensure!(
        domains(base) == domains(current),
        "cannot compare different heap domains"
    );
    let order = b
        .iter()
        .map(|metric| a.iter().position(|m| m == metric).unwrap())
        .collect::<Vec<_>>();
    let current_index = ProfileIndex::new(current);
    let base_index = ProfileIndex::new(base);
    let mut output = current.clone();
    output.samples.clear();
    let mut samples: BTreeMap<String, Sample> = BTreeMap::new();
    for sample in &current.samples {
        let key = current_index.sample_key(sample)?;
        if let Some(existing) = samples.get_mut(&key) {
            add(&mut existing.values, &sample.values, 1)?;
        } else {
            samples.insert(key, sample.clone());
        }
    }
    let strings = base
        .strings
        .strings
        .iter()
        .map(|s| output.strings.intern(s))
        .collect::<Vec<_>>();
    let mut mapping_ids = HashMap::from([(0, 0)]);
    let mut function_ids = HashMap::from([(0, 0)]);
    let mut location_ids = HashMap::new();
    let mut next = output.mappings.iter().map(|m| m.id).max().unwrap_or(0);
    for mapping in &base.mappings {
        let mut m = mapping.clone();
        next = next.checked_add(1).context("pprof identifier overflow")?;
        mapping_ids.insert(m.id, next);
        m.id = next;
        m.filename = strings[m.filename as usize];
        m.build_id = strings[m.build_id as usize];
        output.mappings.push(m);
    }
    next = output.functions.iter().map(|f| f.id).max().unwrap_or(0);
    for function in &base.functions {
        let mut f = function.clone();
        next = next.checked_add(1).context("pprof identifier overflow")?;
        function_ids.insert(f.id, next);
        f.id = next;
        f.name = strings[f.name as usize];
        f.system_name = strings[f.system_name as usize];
        f.filename = strings[f.filename as usize];
        output.functions.push(f);
    }
    next = output.locations.iter().map(|l| l.id).max().unwrap_or(0);
    for location in &base.locations {
        let mut l = location.clone();
        next = next.checked_add(1).context("pprof identifier overflow")?;
        location_ids.insert(l.id, next);
        l.id = next;
        l.mapping_id = *mapping_ids
            .get(&l.mapping_id)
            .context("missing base mapping")?;
        for line in &mut l.lines {
            line.function_id = *function_ids
                .get(&line.function_id)
                .context("missing base function")?;
        }
        output.locations.push(l);
    }
    for sample in &base.samples {
        let key = base_index.sample_key(sample)?;
        let values = order.iter().map(|i| sample.values[*i]).collect::<Vec<_>>();
        if let Some(existing) = samples.get_mut(&key) {
            add(&mut existing.values, &values, -1)?;
        } else {
            let mut s = sample.clone();
            s.values = values
                .iter()
                .map(|v| v.checked_neg().context("heap difference overflows int64"))
                .collect::<Result<_>>()?;
            s.location_ids = s
                .location_ids
                .iter()
                .map(|id| {
                    location_ids
                        .get(id)
                        .copied()
                        .context("missing base location")
                })
                .collect::<Result<_>>()?;
            for label in &mut s.labels {
                label.key = strings[label.key as usize];
                label.str_index = strings[label.str_index as usize];
                label.num_unit = strings[label.num_unit as usize];
            }
            samples.insert(key, s);
        }
    }
    output.samples = samples
        .into_values()
        .filter(|s| s.values.iter().any(|v| *v != 0))
        .collect();
    output.comments.push(
        output
            .strings
            .intern("Signed heap difference: current snapshot minus base snapshot."),
    );
    Ok(output)
}
fn add(target: &mut [i64], source: &[i64], sign: i64) -> Result<()> {
    for (a, b) in target.iter_mut().zip(source) {
        *a = if sign == 1 {
            a.checked_add(*b)
        } else {
            a.checked_sub(*b)
        }
        .context("heap difference overflows int64")?;
    }
    Ok(())
}
struct ProfileIndex<'a> {
    profile: &'a PprofProfile,
    locations: HashMap<u64, &'a Location>,
    functions: HashMap<u64, &'a Function>,
    mappings: HashMap<u64, &'a Mapping>,
}
impl<'a> ProfileIndex<'a> {
    fn new(profile: &'a PprofProfile) -> Self {
        Self {
            profile,
            locations: profile.locations.iter().map(|l| (l.id, l)).collect(),
            functions: profile.functions.iter().map(|f| (f.id, f)).collect(),
            mappings: profile.mappings.iter().map(|m| (m.id, m)).collect(),
        }
    }
    fn sample_key(&self, sample: &Sample) -> Result<String> {
        let profile = self.profile;

        let string = |id: u64| {
            profile
                .strings
                .strings
                .get(id as usize)
                .cloned()
                .context("invalid pprof string index")
        };
        let mut stack = Vec::new();
        for id in &sample.location_ids {
            let l = self.locations.get(id).context("missing pprof location")?;
            let mut lines = Vec::new();
            for line in &l.lines {
                let f = self
                    .functions
                    .get(&line.function_id)
                    .context("missing pprof function")?;
                lines.push((
                    string(f.name)?,
                    string(f.system_name)?,
                    string(f.filename)?,
                    f.start_line,
                    line.line,
                    line.column,
                ));
            }
            let mapping = self.mappings.get(&l.mapping_id);
            let binary = if let Some(m) = mapping {
                if m.build_id != 0 && !string(m.build_id)?.is_empty() {
                    string(m.build_id)?
                } else {
                    string(m.filename)?
                }
            } else {
                String::new()
            };
            let address = if lines.is_empty() || lines.iter().all(|l| l.0.starts_with("0x")) {
                mapping
                    .map(|m| {
                        l.address
                            .saturating_sub(m.memory_start)
                            .saturating_add(m.file_offset)
                    })
                    .unwrap_or(l.address)
            } else {
                0
            };
            for line in &mut lines {
                if line.0.starts_with("0x") {
                    line.0 = "[unknown]".into();
                }
                if line.1.starts_with("0x") {
                    line.1 = "[unknown]".into();
                }
            }
            stack.push((binary, address, lines));
        }
        let mut labels = sample
            .labels
            .iter()
            .map(|l| {
                Ok((
                    string(l.key)?,
                    string(l.str_index)?,
                    l.num,
                    string(l.num_unit)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        labels.sort();
        Ok(serde_json::to_string(&(stack, labels))?)
    }
}
