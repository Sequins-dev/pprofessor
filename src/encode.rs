//! Build a pprof protobuf from a [`SymbolicatedProfile`].

use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};

use bytes::Bytes;

use crate::pprof::{Function, Label, Line, Location, Mapping, ProfileEncoder, Sample, ValueType};
use crate::symbolicated::SymbolicatedProfile;

pub fn build_proto(profile: &SymbolicatedProfile) -> Bytes {
    let mut enc = ProfileEncoder::new();

    // Value types: samples/count and aggregate sampled thread wall time.
    let s_samples = enc.strings.intern("samples");
    let s_count = enc.strings.intern("count");
    let s_wall = enc.strings.intern("wall");
    let s_nanoseconds = enc.strings.intern("nanoseconds");
    let s_thread_id = enc.strings.intern("thread_id");
    let s_thread_name = enc.strings.intern("thread_name");
    let s_timestamp = enc.strings.intern("pprofessor::timestamp");
    let s_nanoseconds_label = enc.strings.intern("nanoseconds");

    enc.value_types.push(ValueType {
        r#type: s_samples,
        unit: s_count,
    });
    enc.value_types.push(ValueType {
        r#type: s_wall,
        unit: s_nanoseconds,
    });

    let period_ns = 1_000_000_000i64 / profile.freq_hz as i64;
    enc.period_type = ValueType {
        r#type: s_wall,
        unit: s_nanoseconds,
    };
    enc.period = period_ns;

    enc.duration_nanos = profile.duration.as_nanos() as i64;
    enc.time_nanos = profile
        .start_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_nanos() as i64;

    let mut images = profile.images.clone();
    images.sort_by_key(|image| image.load_address);
    for (index, image) in images.iter().enumerate() {
        let filename = enc.strings.intern(&image.path);
        let memory_limit = images
            .get(index + 1)
            .map(|next| next.load_address)
            .or_else(|| {
                std::fs::metadata(&image.path)
                    .ok()
                    .map(|metadata| image.load_address.saturating_add(metadata.len()))
            })
            .unwrap_or(image.load_address.saturating_add(1));
        enc.mappings.push(Mapping {
            id: index as u64 + 1,
            memory_start: image.load_address,
            memory_limit,
            file_offset: 0,
            filename,
            build_id: 0,
        });
    }

    // Deduplicate functions and locations by address.
    let mut func_id_map: HashMap<u64, u64> = HashMap::new(); // addr -> function_id
    let mut loc_id_map: HashMap<u64, u64> = HashMap::new(); // addr -> location_id

    for sample in &profile.samples {
        let mut location_ids: Vec<u64> = Vec::with_capacity(sample.stack.len());

        for &addr in &sample.stack {
            let Some(frame) = profile.frames.get(&addr) else {
                continue;
            };

            let loc_id = *loc_id_map.entry(addr).or_insert_with(|| {
                let func_id = *func_id_map.entry(addr).or_insert_with(|| {
                    let fid = enc.functions.len() as u64 + 1;
                    let name_idx = enc.strings.intern(&frame.function);
                    let file_idx = enc.strings.intern(&frame.file);
                    enc.functions.push(Function {
                        id: fid,
                        name: name_idx,
                        system_name: name_idx,
                        filename: file_idx,
                        start_line: 0,
                    });
                    fid
                });

                let lid = enc.locations.len() as u64 + 1;
                enc.locations.push(Location {
                    id: lid,
                    mapping_id: images.partition_point(|image| image.load_address <= addr) as u64,
                    address: addr,
                    lines: vec![Line {
                        function_id: func_id,
                        line: frame.line as i64,
                    }],
                });
                lid
            });

            location_ids.push(loc_id);
        }

        if location_ids.is_empty() {
            continue;
        }

        let thread_name = profile
            .threads
            .get(&sample.thread_id)
            .cloned()
            .unwrap_or_default();

        let mut labels = vec![Label {
            key: s_thread_id,
            str_index: 0,
            num: sample.thread_id as i64,
            num_unit: 0,
        }];

        if !thread_name.is_empty() {
            let name_idx = enc.strings.intern(&thread_name);
            labels.push(Label {
                key: s_thread_name,
                str_index: name_idx,
                num: 0,
                num_unit: 0,
            });
        }

        if let Some(timestamp_nanos) = sample.timestamp_nanos {
            labels.push(Label {
                key: s_timestamp,
                str_index: 0,
                num: timestamp_nanos.min(i64::MAX as u64) as i64,
                num_unit: s_nanoseconds_label,
            });
        }

        let count = sample.count.min(i64::MAX as u64) as i64;
        enc.samples.push(Sample {
            location_ids,
            values: vec![count, count.saturating_mul(period_ns)],
            labels,
        });
    }

    Bytes::from(enc.encode())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbolicated::{Sample, StackFrame, SymbolicatedProfile};
    use std::collections::HashMap;
    use std::time::{Duration, SystemTime};

    fn read_varint(data: &[u8], offset: &mut usize) -> u64 {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = data[*offset];
            *offset += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
            shift += 7;
        }
    }

    fn length_delimited_fields(data: &[u8], wanted_field: u64) -> Vec<&[u8]> {
        let mut offset = 0;
        let mut fields = Vec::new();
        while offset < data.len() {
            let tag = read_varint(data, &mut offset);
            let field = tag >> 3;
            match tag & 0x7 {
                0 => {
                    let _ = read_varint(data, &mut offset);
                }
                2 => {
                    let length = read_varint(data, &mut offset) as usize;
                    let value = &data[offset..offset + length];
                    if field == wanted_field {
                        fields.push(value);
                    }
                    offset += length;
                }
                wire_type => panic!("unsupported protobuf wire type {wire_type}"),
            }
        }
        fields
    }

    fn packed_int64s(data: &[u8]) -> Vec<i64> {
        let mut offset = 0;
        let mut values = Vec::new();
        while offset < data.len() {
            values.push(read_varint(data, &mut offset) as i64);
        }
        values
    }

    fn dummy_profile() -> SymbolicatedProfile {
        let mut frames = HashMap::new();
        frames.insert(
            0x1000u64,
            StackFrame {
                address: 0x1000,
                function: "leaf_fn".to_string(),
                file: "lib.rs".to_string(),
                line: 42,
            },
        );
        frames.insert(
            0x2000u64,
            StackFrame {
                address: 0x2000,
                function: "main".to_string(),
                file: "main.rs".to_string(),
                line: 10,
            },
        );

        let mut threads = HashMap::new();
        threads.insert(1u64, "main".to_string());

        SymbolicatedProfile {
            images: Vec::new(),
            frames,
            threads,
            samples: vec![Sample {
                thread_id: 1,
                stack: vec![0x1000, 0x2000],
                count: 5,
                timestamp_nanos: Some(123_456_789),
            }],
            start_time: SystemTime::now(),
            duration: Duration::from_secs(1),
            freq_hz: 99,
        }
    }

    #[test]
    fn test_build_proto_nonempty() {
        let profile = dummy_profile();
        let proto = build_proto(&profile);
        assert!(!proto.is_empty());
        // First byte should be field 1, wire type 2 (0x0a) — sample_type
        assert_eq!(proto[0], 0x0a);
    }

    #[test]
    fn test_build_proto_includes_timestamp_label() {
        let proto = build_proto(&dummy_profile());
        assert!(
            proto
                .windows(b"pprofessor::timestamp".len())
                .any(|window| window == b"pprofessor::timestamp")
        );
        assert!(
            proto
                .windows(b"nanoseconds".len())
                .any(|window| window == b"nanoseconds")
        );
    }

    #[test]
    fn test_build_proto_includes_wall_time_values() {
        let proto = build_proto(&dummy_profile());
        assert!(proto.windows(b"wall".len()).any(|window| window == b"wall"));

        let sample_types = length_delimited_fields(&proto, 1);
        assert_eq!(sample_types.len(), 2);

        let samples = length_delimited_fields(&proto, 2);
        assert_eq!(samples.len(), 1);
        let packed_values = length_delimited_fields(samples[0], 2);
        assert_eq!(packed_values.len(), 1);

        let period_ns = 1_000_000_000i64 / 99;
        assert_eq!(packed_int64s(packed_values[0]), vec![5, 5 * period_ns]);
    }
}
