//! pprof protobuf encoder — port of DataDog/pprof-format (MIT).
//!
//! Only two wire types are used (sufficient for the pprof schema):
//!   - wire type 0: varint (i64, u64, bool)
//!   - wire type 2: length-delimited (submessages, packed repeated, strings)
//!
//! All pprof field numbers are < 16, so every tag fits in a single byte.

use std::collections::HashMap;

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

pub struct StringTable {
    pub strings: Vec<String>,
    indices: HashMap<String, u64>,
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
}

// ---------------------------------------------------------------------------
// pprof message types
// ---------------------------------------------------------------------------

pub struct ValueType {
    pub r#type: u64,
    pub unit: u64,
}

impl ValueType {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint_field(buf, 1, self.r#type);
        encode_varint_field(buf, 2, self.unit);
    }
}

pub struct Line {
    pub function_id: u64,
    pub line: i64,
}

impl Line {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint_field(buf, 1, self.function_id);
        encode_sint64_field(buf, 2, self.line);
    }
}

pub struct Location {
    pub id: u64,
    pub lines: Vec<Line>,
}

impl Location {
    fn encode(&self, buf: &mut Vec<u8>) {
        encode_varint_field(buf, 1, self.id);
        for line in &self.lines {
            let mut inner = Vec::new();
            line.encode(&mut inner);
            encode_length_delimited(buf, 4, &inner);
        }
    }
}

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
}

/// A pprof Sample label (key/value pair attached to a sample).
///
/// Either `str_index` (a string table index) or `num` should be non-zero,
/// not both.
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
}

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
}

// ---------------------------------------------------------------------------
// Profile encoder
// ---------------------------------------------------------------------------

pub struct ProfileEncoder {
    pub strings: StringTable,
    pub value_types: Vec<ValueType>,
    pub samples: Vec<Sample>,
    pub locations: Vec<Location>,
    pub functions: Vec<Function>,
    pub time_nanos: i64,
    pub duration_nanos: i64,
    pub period_type: ValueType,
    pub period: i64,
}

impl Default for ProfileEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileEncoder {
    pub fn new() -> Self {
        ProfileEncoder {
            strings: StringTable::new(),
            value_types: Vec::new(),
            samples: Vec::new(),
            locations: Vec::new(),
            functions: Vec::new(),
            time_nanos: 0,
            duration_nanos: 0,
            period_type: ValueType { r#type: 0, unit: 0 },
            period: 0,
        }
    }

    pub fn encode(self) -> Vec<u8> {
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

        buf
    }
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
}
