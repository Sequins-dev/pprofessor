//! Reversible pprof profile model and protobuf codec.
//!
//! The encoder only needs two wire types:
//!   - wire type 0: varint (i64, u64, bool)
//!   - wire type 2: length-delimited (submessages, packed repeated, strings)
//!
//! All pprof field numbers are < 16, so every tag fits in a single byte.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// Protobuf primitives
// ---------------------------------------------------------------------------

fn encode_varint(buf: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            buf.push(byte);
            break;
        }
        buf.push(byte | 0x80);
    }
}

fn encode_varint_field(buf: &mut Vec<u8>, field: u32, value: u64) {
    if value == 0 {
        return;
    }
    buf.push((field << 3) as u8); // wire type 0
    encode_varint(buf, value);
}

fn encode_sint64_field(buf: &mut Vec<u8>, field: u32, value: i64) {
    encode_varint_field(buf, field, value as u64);
}

fn encode_length_delimited(buf: &mut Vec<u8>, field: u32, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    buf.push(((field << 3) | 2) as u8); // wire type 2
    encode_varint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

fn encode_string_field(buf: &mut Vec<u8>, field: u32, s: &str) {
    // Always emit even for empty strings — pprof requires string_table[0] == "".
    buf.push(((field << 3) | 2) as u8);
    encode_varint(buf, s.len() as u64);
    buf.extend_from_slice(s.as_bytes());
}

fn encode_packed_u64(buf: &mut Vec<u8>, field: u32, values: &[u64]) {
    if values.is_empty() {
        return;
    }
    let mut inner = Vec::new();
    for &v in values {
        encode_varint(&mut inner, v);
    }
    encode_length_delimited(buf, field, &inner);
}

fn encode_packed_i64(buf: &mut Vec<u8>, field: u32, values: &[i64]) {
    if values.is_empty() {
        return;
    }
    let mut inner = Vec::new();
    for &v in values {
        encode_varint(&mut inner, v as u64);
    }
    encode_length_delimited(buf, field, &inner);
}

// ---------------------------------------------------------------------------
// StringTable
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringTable {
    pub strings: Vec<String>,
    indices: HashMap<String, u64>,
}

impl Serialize for StringTable {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.strings.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StringTable {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let strings = Vec::<String>::deserialize(deserializer)?;
        Self::from_strings(strings).map_err(serde::de::Error::custom)
    }
}

impl Default for StringTable {
    fn default() -> Self {
        Self::new()
    }
}

impl StringTable {
    pub fn new() -> Self {
        let mut st = StringTable {
            strings: Vec::new(),
            indices: HashMap::new(),
        };
        st.intern(""); // index 0 must be empty string (pprof requirement)
        st
    }

    pub fn intern(&mut self, s: &str) -> u64 {
        if let Some(&idx) = self.indices.get(s) {
            return idx;
        }
        let idx = self.strings.len() as u64;
        self.strings.push(s.to_string());
        self.indices.insert(s.to_string(), idx);
        idx
    }

    fn from_strings(strings: Vec<String>) -> Result<Self> {
        if strings.first().is_none_or(|value| !value.is_empty()) {
            bail!("invalid pprof string table: index 0 must be empty");
        }
        let mut indices = HashMap::new();
        for (index, value) in strings.iter().enumerate() {
            indices.entry(value.clone()).or_insert(index as u64);
        }
        Ok(Self { strings, indices })
    }
}

// ---------------------------------------------------------------------------
// pprof message types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ValueType {
    #[serde(rename = "type")]
    pub r#type: u64,
    pub unit: u64,
}

impl ValueType {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint_field(buf, 1, self.r#type);
        encode_varint_field(buf, 2, self.unit);
    }

    fn decode(data: &[u8]) -> Result<Self> {
        let mut value = Self::default();
        let mut fields = Fields::new(data);
        while let Some(field) = fields.next()? {
            match (field.number, field.value) {
                (1, FieldValue::Varint(decoded)) => value.r#type = decoded,
                (2, FieldValue::Varint(decoded)) => value.unit = decoded,
                _ => {}
            }
        }
        Ok(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Line {
    pub function_id: u64,
    pub line: i64,
    pub column: i64,
}

impl Line {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint_field(buf, 1, self.function_id);
        encode_sint64_field(buf, 2, self.line);
        encode_sint64_field(buf, 3, self.column);
    }

    fn decode(data: &[u8]) -> Result<Self> {
        let mut line = Self::default();
        let mut fields = Fields::new(data);
        while let Some(field) = fields.next()? {
            match (field.number, field.value) {
                (1, FieldValue::Varint(value)) => line.function_id = value,
                (2, FieldValue::Varint(value)) => line.line = value as i64,
                (3, FieldValue::Varint(value)) => line.column = value as i64,
                _ => {}
            }
        }
        Ok(line)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Location {
    pub id: u64,
    pub mapping_id: u64,
    pub address: u64,
    pub lines: Vec<Line>,
    pub is_folded: bool,
}

impl Location {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint_field(buf, 1, self.id);
        encode_varint_field(buf, 2, self.mapping_id);
        encode_varint_field(buf, 3, self.address);
        for line in &self.lines {
            let mut inner = Vec::new();
            line.encode(&mut inner);
            encode_length_delimited(buf, 4, &inner);
        }
        encode_varint_field(buf, 5, u64::from(self.is_folded));
    }

    fn decode(data: &[u8]) -> Result<Self> {
        let mut location = Self::default();
        let mut fields = Fields::new(data);
        while let Some(field) = fields.next()? {
            match (field.number, field.value) {
                (1, FieldValue::Varint(value)) => location.id = value,
                (2, FieldValue::Varint(value)) => location.mapping_id = value,
                (3, FieldValue::Varint(value)) => location.address = value,
                (4, FieldValue::Bytes(value)) => location.lines.push(Line::decode(value)?),
                (5, FieldValue::Varint(value)) => location.is_folded = value != 0,
                _ => {}
            }
        }
        Ok(location)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Mapping {
    pub id: u64,
    pub memory_start: u64,
    pub memory_limit: u64,
    pub file_offset: u64,
    pub filename: u64,
    pub build_id: u64,
    pub has_functions: bool,
    pub has_filenames: bool,
    pub has_line_numbers: bool,
    pub has_inline_frames: bool,
}

impl Mapping {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint_field(buf, 1, self.id);
        encode_varint_field(buf, 2, self.memory_start);
        encode_varint_field(buf, 3, self.memory_limit);
        encode_varint_field(buf, 4, self.file_offset);
        encode_varint_field(buf, 5, self.filename);
        encode_varint_field(buf, 6, self.build_id);
        encode_varint_field(buf, 7, u64::from(self.has_functions));
        encode_varint_field(buf, 8, u64::from(self.has_filenames));
        encode_varint_field(buf, 9, u64::from(self.has_line_numbers));
        encode_varint_field(buf, 10, u64::from(self.has_inline_frames));
    }

    fn decode(data: &[u8]) -> Result<Self> {
        let mut mapping = Self::default();
        let mut fields = Fields::new(data);
        while let Some(field) = fields.next()? {
            let FieldValue::Varint(value) = field.value else {
                continue;
            };
            match field.number {
                1 => mapping.id = value,
                2 => mapping.memory_start = value,
                3 => mapping.memory_limit = value,
                4 => mapping.file_offset = value,
                5 => mapping.filename = value,
                6 => mapping.build_id = value,
                7 => mapping.has_functions = value != 0,
                8 => mapping.has_filenames = value != 0,
                9 => mapping.has_line_numbers = value != 0,
                10 => mapping.has_inline_frames = value != 0,
                _ => {}
            }
        }
        Ok(mapping)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Function {
    pub id: u64,
    pub name: u64,
    pub system_name: u64,
    pub filename: u64,
    pub start_line: i64,
}

impl Function {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint_field(buf, 1, self.id);
        encode_varint_field(buf, 2, self.name);
        encode_varint_field(buf, 3, self.system_name);
        encode_varint_field(buf, 4, self.filename);
        encode_sint64_field(buf, 5, self.start_line);
    }

    fn decode(data: &[u8]) -> Result<Self> {
        let mut function = Self::default();
        let mut fields = Fields::new(data);
        while let Some(field) = fields.next()? {
            let FieldValue::Varint(value) = field.value else {
                continue;
            };
            match field.number {
                1 => function.id = value,
                2 => function.name = value,
                3 => function.system_name = value,
                4 => function.filename = value,
                5 => function.start_line = value as i64,
                _ => {}
            }
        }
        Ok(function)
    }
}

/// A pprof Sample label (key/value pair attached to a sample).
///
/// Either `str_index` (a string table index) or `num` should be non-zero,
/// not both.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Label {
    /// String table index for the label key.
    pub key: u64,
    /// String table index for a string value (0 = not a string label).
    pub str_index: u64,
    /// Numeric value (0 = not a numeric label).
    pub num: i64,
    /// String table index for the unit of `num` (0 = no unit).
    pub num_unit: u64,
}

impl Label {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint_field(buf, 1, self.key);
        encode_varint_field(buf, 2, self.str_index);
        encode_sint64_field(buf, 3, self.num);
        encode_varint_field(buf, 4, self.num_unit);
    }

    fn decode(data: &[u8]) -> Result<Self> {
        let mut label = Self::default();
        let mut fields = Fields::new(data);
        while let Some(field) = fields.next()? {
            let FieldValue::Varint(value) = field.value else {
                continue;
            };
            match field.number {
                1 => label.key = value,
                2 => label.str_index = value,
                3 => label.num = value as i64,
                4 => label.num_unit = value,
                _ => {}
            }
        }
        Ok(label)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Sample {
    pub location_ids: Vec<u64>,
    pub values: Vec<i64>,
    pub labels: Vec<Label>,
}

impl Sample {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_packed_u64(buf, 1, &self.location_ids);
        encode_packed_i64(buf, 2, &self.values);
        for label in &self.labels {
            let mut inner = Vec::new();
            label.encode(&mut inner);
            encode_length_delimited(buf, 3, &inner);
        }
    }

    fn decode(data: &[u8]) -> Result<Self> {
        let mut sample = Self::default();
        let mut fields = Fields::new(data);
        while let Some(field) = fields.next()? {
            match (field.number, field.value) {
                (1, FieldValue::Bytes(value)) => {
                    decode_packed(value, |value| sample.location_ids.push(value))?
                }
                (1, FieldValue::Varint(value)) => sample.location_ids.push(value),
                (2, FieldValue::Bytes(value)) => {
                    decode_packed(value, |value| sample.values.push(value as i64))?
                }
                (2, FieldValue::Varint(value)) => sample.values.push(value as i64),
                (3, FieldValue::Bytes(value)) => sample.labels.push(Label::decode(value)?),
                _ => {}
            }
        }
        Ok(sample)
    }
}

// ---------------------------------------------------------------------------
// Canonical profile model and codec
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PprofProfile {
    pub strings: StringTable,
    pub value_types: Vec<ValueType>,
    pub samples: Vec<Sample>,
    pub mappings: Vec<Mapping>,
    pub locations: Vec<Location>,
    pub functions: Vec<Function>,
    pub time_nanos: i64,
    pub duration_nanos: i64,
    pub period_type: ValueType,
    pub period: i64,
    pub drop_frames: u64,
    pub keep_frames: u64,
    pub comments: Vec<u64>,
    pub default_sample_type: u64,
}

/// Compatibility name for the original write-only pprof model.
pub type ProfileEncoder = PprofProfile;

impl Default for PprofProfile {
    fn default() -> Self {
        Self::new()
    }
}

impl PprofProfile {
    /// Create an empty valid pprof profile with string table index 0 reserved.
    pub fn new() -> Self {
        PprofProfile {
            strings: StringTable::new(),
            value_types: Vec::new(),
            samples: Vec::new(),
            mappings: Vec::new(),
            locations: Vec::new(),
            functions: Vec::new(),
            time_nanos: 0,
            duration_nanos: 0,
            period_type: ValueType::default(),
            period: 0,
            drop_frames: 0,
            keep_frames: 0,
            comments: Vec::new(),
            default_sample_type: 0,
        }
    }

    /// Encode this model as a raw (uncompressed) pprof protobuf.
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();

        // field 1: sample_type (repeated ValueType)
        for vt in &self.value_types {
            let mut inner = Vec::new();
            vt.encode(&mut inner);
            encode_length_delimited(&mut buf, 1, &inner);
        }

        // field 2: sample (repeated Sample)
        for s in &self.samples {
            let mut inner = Vec::new();
            s.encode(&mut inner);
            encode_length_delimited(&mut buf, 2, &inner);
        }

        // field 3: mapping (repeated Mapping)
        for mapping in &self.mappings {
            let mut inner = Vec::new();
            mapping.encode(&mut inner);
            encode_length_delimited(&mut buf, 3, &inner);
        }

        // field 4: location (repeated Location)
        for loc in &self.locations {
            let mut inner = Vec::new();
            loc.encode(&mut inner);
            encode_length_delimited(&mut buf, 4, &inner);
        }

        // field 5: function (repeated Function)
        for f in &self.functions {
            let mut inner = Vec::new();
            f.encode(&mut inner);
            encode_length_delimited(&mut buf, 5, &inner);
        }

        // field 6: string_table (repeated string)
        for s in &self.strings.strings {
            encode_string_field(&mut buf, 6, s);
        }

        // fields 7-8: frame filters
        encode_varint_field(&mut buf, 7, self.drop_frames);
        encode_varint_field(&mut buf, 8, self.keep_frames);

        // field 9: time_nanos
        encode_sint64_field(&mut buf, 9, self.time_nanos);

        // field 10: duration_nanos
        encode_sint64_field(&mut buf, 10, self.duration_nanos);

        // field 11: period_type (ValueType)
        {
            let mut inner = Vec::new();
            self.period_type.encode(&mut inner);
            encode_length_delimited(&mut buf, 11, &inner);
        }

        // field 12: period
        encode_sint64_field(&mut buf, 12, self.period);

        // field 13: comments
        for &comment in &self.comments {
            encode_varint_field(&mut buf, 13, comment);
        }

        // field 14: default sample type
        encode_varint_field(&mut buf, 14, self.default_sample_type);

        buf
    }

    /// Decode a raw (uncompressed) pprof protobuf into the canonical model.
    pub fn decode(data: &[u8]) -> Result<Self> {
        let mut profile = Self::new();
        profile.strings = StringTable {
            strings: Vec::new(),
            indices: HashMap::new(),
        };

        let mut fields = Fields::new(data);
        while let Some(field) = fields.next()? {
            match (field.number, field.value) {
                (1, FieldValue::Bytes(value)) => {
                    profile.value_types.push(ValueType::decode(value)?)
                }
                (2, FieldValue::Bytes(value)) => profile.samples.push(Sample::decode(value)?),
                (3, FieldValue::Bytes(value)) => profile.mappings.push(Mapping::decode(value)?),
                (4, FieldValue::Bytes(value)) => profile.locations.push(Location::decode(value)?),
                (5, FieldValue::Bytes(value)) => profile.functions.push(Function::decode(value)?),
                (6, FieldValue::Bytes(value)) => profile.strings.strings.push(
                    std::str::from_utf8(value)
                        .context("pprof string table contains invalid UTF-8")?
                        .to_owned(),
                ),
                (7, FieldValue::Varint(value)) => profile.drop_frames = value,
                (8, FieldValue::Varint(value)) => profile.keep_frames = value,
                (9, FieldValue::Varint(value)) => profile.time_nanos = value as i64,
                (10, FieldValue::Varint(value)) => profile.duration_nanos = value as i64,
                (11, FieldValue::Bytes(value)) => profile.period_type = ValueType::decode(value)?,
                (12, FieldValue::Varint(value)) => profile.period = value as i64,
                (13, FieldValue::Bytes(value)) => {
                    decode_packed(value, |value| profile.comments.push(value))?
                }
                (13, FieldValue::Varint(value)) => profile.comments.push(value),
                (14, FieldValue::Varint(value)) => profile.default_sample_type = value,
                _ => {}
            }
        }
        profile.strings = StringTable::from_strings(profile.strings.strings)?;
        Ok(profile)
    }
}

fn decode_packed(data: &[u8], mut visit: impl FnMut(u64)) -> Result<()> {
    let mut offset = 0;
    while offset < data.len() {
        visit(read_varint(data, &mut offset)?);
    }
    Ok(())
}

#[derive(Debug)]
struct Field<'a> {
    number: u64,
    value: FieldValue<'a>,
}

#[derive(Debug)]
enum FieldValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
    Fixed32,
    Fixed64,
}

struct Fields<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Fields<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn next(&mut self) -> Result<Option<Field<'a>>> {
        if self.offset == self.data.len() {
            return Ok(None);
        }
        let tag = read_varint(self.data, &mut self.offset)?;
        let number = tag >> 3;
        if number == 0 {
            bail!("invalid protobuf field number 0");
        }
        let value = match tag & 0x07 {
            0 => FieldValue::Varint(read_varint(self.data, &mut self.offset)?),
            1 => {
                self.take(8)?;
                FieldValue::Fixed64
            }
            2 => {
                let length = read_varint(self.data, &mut self.offset)?;
                let length =
                    usize::try_from(length).context("protobuf field length does not fit usize")?;
                FieldValue::Bytes(self.take(length)?)
            }
            5 => {
                self.take(4)?;
                FieldValue::Fixed32
            }
            wire => bail!("unsupported protobuf wire type {wire}"),
        };
        Ok(Some(Field { number, value }))
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(length)
            .context("protobuf field length overflow")?;
        let value = self
            .data
            .get(self.offset..end)
            .context("truncated protobuf field")?;
        self.offset = end;
        Ok(value)
    }
}

fn read_varint(data: &[u8], offset: &mut usize) -> Result<u64> {
    let mut value = 0u64;
    for shift in (0..=63).step_by(7) {
        let byte = *data.get(*offset).context("truncated protobuf varint")?;
        *offset += 1;
        if shift == 63 && byte > 1 {
            bail!("protobuf varint overflow");
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("protobuf varint overflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_table_empty_at_zero() {
        let st = StringTable::new();
        assert_eq!(st.strings[0], "");
    }

    #[test]
    fn test_intern_deduplication() {
        let mut st = StringTable::new();
        let a = st.intern("hello");
        let b = st.intern("hello");
        assert_eq!(a, b);
    }

    #[test]
    fn test_encode_nonempty() {
        let mut enc = ProfileEncoder::new();
        let samples_str = enc.strings.intern("samples");
        let count_str = enc.strings.intern("count");
        enc.value_types.push(ValueType {
            r#type: samples_str,
            unit: count_str,
        });
        enc.period_type = ValueType {
            r#type: samples_str,
            unit: count_str,
        };
        enc.period = 10_000_000;
        enc.time_nanos = 1_000_000_000;
        enc.duration_nanos = 2_000_000_000;
        let buf = enc.encode();
        // Should produce non-empty output
        assert!(!buf.is_empty());
        // First byte should be a valid protobuf tag (field 1, wire type 2 = 0x0a)
        assert_eq!(buf[0], 0x0a);
    }

    #[test]
    fn test_encodes_mapping_and_location_identity() {
        let mut enc = ProfileEncoder::new();
        let filename = enc.strings.intern("/tmp/example");
        let build_id = enc.strings.intern("ABCDEF");
        enc.mappings.push(Mapping {
            id: 7,
            memory_start: 0x1000,
            memory_limit: 0x2000,
            file_offset: 0,
            filename,
            build_id,
            ..Mapping::default()
        });
        enc.locations.push(Location {
            id: 9,
            mapping_id: 7,
            address: 0x1234,
            lines: Vec::new(),
            ..Location::default()
        });

        let bytes = enc.encode();
        assert!(bytes.windows(2).any(|window| window == [0x10, 0x07]));
        assert!(bytes.windows(3).any(|window| window == [0x18, 0xb4, 0x24]));
    }

    #[test]
    fn protobuf_round_trip_preserves_the_canonical_profile_model() {
        let mut profile = PprofProfile::new();
        let samples = profile.strings.intern("samples");
        let count = profile.strings.intern("count");
        let function_name = profile.strings.intern("work");
        let filename = profile.strings.intern("src/work.rs");
        let label_key = profile.strings.intern("thread");
        let label_value = profile.strings.intern("main");

        profile.value_types.push(ValueType {
            r#type: samples,
            unit: count,
        });
        profile.samples.push(Sample {
            location_ids: vec![11],
            values: vec![7],
            labels: vec![Label {
                key: label_key,
                str_index: label_value,
                num: 0,
                num_unit: 0,
            }],
        });
        profile.mappings.push(Mapping {
            id: 3,
            memory_start: 0x1000,
            memory_limit: 0x2000,
            file_offset: 0,
            filename,
            build_id: 0,
            has_functions: true,
            has_filenames: true,
            has_line_numbers: true,
            has_inline_frames: true,
        });
        profile.locations.push(Location {
            id: 11,
            mapping_id: 3,
            address: 0x1234,
            lines: vec![Line {
                function_id: 5,
                line: 42,
                column: 9,
            }],
            is_folded: true,
        });
        profile.functions.push(Function {
            id: 5,
            name: function_name,
            system_name: function_name,
            filename,
            start_line: 40,
        });
        profile.drop_frames = function_name;
        profile.keep_frames = filename;
        profile.time_nanos = 1_000;
        profile.duration_nanos = 2_000;
        profile.period_type = ValueType {
            r#type: samples,
            unit: count,
        };
        profile.period = 10;
        profile.comments = vec![label_value];
        profile.default_sample_type = samples;

        let encoded = profile.encode();
        let decoded = PprofProfile::decode(&encoded).unwrap();

        assert_eq!(decoded, profile);
    }

    #[test]
    fn serde_round_trip_rebuilds_the_string_lookup_cache() {
        let mut profile = PprofProfile::new();
        let existing = profile.strings.intern("existing");
        profile.default_sample_type = existing;

        let json = serde_json::to_string(&profile).unwrap();
        let mut decoded: PprofProfile = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, profile);
        assert_eq!(decoded.strings.intern("existing"), existing);
        assert_eq!(decoded.strings.strings, ["", "existing"]);
    }
}
