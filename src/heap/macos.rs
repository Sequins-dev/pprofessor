//! Adapter for Apple's documented malloc_history tool. No private MSL ABI.
use super::{HeapFrame, HeapMapping, HeapProfile, NativeHeapTracker};
use anyhow::{Context, Result, bail, ensure};
use std::io::BufRead;

/// Parse a complete `malloc_history -allEvents` report. VM events are backing
/// storage, not extra malloc objects, and deliberately remain outside the heap.
pub fn parse_history(text: &str) -> Result<HeapProfile> {
    parse_reader(std::io::Cursor::new(text.as_bytes()))
}

/// Consume history incrementally: full MSL logs can be much larger than the
/// resulting pprof profile. Keep stack/allocator state, not the raw event text.
pub fn parse_reader(reader: impl BufRead) -> Result<HeapProfile> {
    let mut lines = reader.lines();
    let header = lines
        .next()
        .transpose()?
        .context("missing malloc_history report header")?;
    ensure!(
        header.starts_with("malloc_history Report Version:  2.0"),
        "unsupported malloc_history report header"
    );
    let mut tracker = NativeHeapTracker::new("native");
    let mut initial_frees = 0u64;
    let mut section = false;
    let mut images_section = false;
    let mut images = std::collections::BTreeMap::new();
    for line in lines {
        let line = line?;
        if images_section {
            if let Some(image) = parse_image(&line) {
                images.insert(image.memory_start, image);
            }
            continue;
        }
        if line == "Binary Images:" {
            images_section = true;
            continue;
        }
        if line == "----" {
            section = true;
            continue;
        }
        if !section {
            continue;
        }
        if line.starts_with("ALLOC ") {
            let (record, stack) = line
                .split_once(": ")
                .context("malformed malloc allocation")?;
            let address = record
                .split_whitespace()
                .nth(1)
                .context("missing allocation address")?
                .split('-')
                .next()
                .unwrap();
            let size = record
                .split_once("[size=")
                .context("missing allocation size")?
                .1
                .strip_suffix(']')
                .context("malformed allocation size")?
                .parse::<u64>()?;
            tracker.allocate(hex(address)?, size, parse_stack(stack)?)?;
        } else if line.starts_with("FREE ") {
            let address = line
                .split_whitespace()
                .nth(1)
                .context("missing free address")?
                .trim_end_matches(':');
            // MSL starts after dyld's earliest allocations. Their frees can
            // appear without a recorded origin; disclose this coverage gap.
            if tracker.release(hex(address)?).is_err() {
                initial_frees += 1;
            }
        } else if !line.trim().is_empty()
            && !line.starts_with("VM_ALLOC ")
            && !line.starts_with("VM_DEALLOC ")
        {
            bail!(
                "unsupported malloc_history event format: {}",
                line.split_whitespace().next().unwrap_or("unknown")
            );
        }
    }
    ensure!(
        section && images_section,
        "incomplete malloc_history report"
    );
    let mut profile = tracker.snapshot()?;
    profile.samples = profile
        .samples
        .into_iter()
        .map(|(stack, values)| {
            let stack = stack
                .into_iter()
                .map(|mut frame| {
                    frame.mapping = images
                        .range(..=frame.address)
                        .next_back()
                        .map(|(_, m)| m)
                        .filter(|m| frame.address < m.memory_limit)
                        .cloned();
                    frame
                })
                .collect();
            (stack, values)
        })
        .collect();
    profile.comment = format!(
        "Exact Apple MallocStackLogging allocation history. Bytes are allocator-reported block sizes. Coverage starts when stack logging is enabled; {initial_frees} frees had no recorded allocation. VM backing regions are excluded."
    );
    Ok(profile)
}
fn hex(value: &str) -> Result<u64> {
    Ok(u64::from_str_radix(
        value
            .strip_prefix("0x")
            .context("expected hexadecimal address")?,
        16,
    )?)
}
fn parse_stack(text: &str) -> Result<Vec<HeapFrame>> {
    let mut frames = Vec::new();
    for frame in text.trim().rsplit(" | ") {
        let (address, rest) = frame
            .split_once(" (")
            .context("malformed allocation stack")?;
        let (_, function) = rest
            .split_once(") ")
            .context("missing allocation frame name")?;
        let (function, file, line) = function
            .rsplit_once(' ')
            .and_then(|(name, source)| {
                source.rsplit_once(':').and_then(|(file, line)| {
                    line.parse::<i64>()
                        .ok()
                        .map(|line| (name.trim_end().to_owned(), file.to_owned(), line))
                })
            })
            .unwrap_or_else(|| (function.to_owned(), String::new(), 0));
        frames.push(HeapFrame {
            address: hex(address)?,
            function,
            file,
            line,
            mapping: None,
        });
    }
    ensure!(!frames.is_empty(), "missing allocation stack");
    Ok(frames)
}
fn parse_image(line: &str) -> Option<HeapMapping> {
    let words = line.split_whitespace().collect::<Vec<_>>();
    if words.len() < 6 || words[1] != "-" {
        return None;
    }
    let memory_start = hex(words[0]).ok()?;
    let memory_limit = hex(words[2]).ok()?.checked_add(1)?;
    let uuid = line.split_once('<').and_then(|(_, s)| s.split_once('>'));
    let (build_id, path) = uuid
        .map(|(id, path)| (id.to_owned(), path.trim().to_owned()))
        .unwrap_or_else(|| (String::new(), words.last().unwrap().to_string()));
    Some(HeapMapping {
        memory_start,
        memory_limit,
        file_offset: 0,
        filename: path,
        build_id,
    })
}
