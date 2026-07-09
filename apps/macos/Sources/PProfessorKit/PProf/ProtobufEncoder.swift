import Foundation

// Port of the Rust pprof encoder. Only two wire types:
//   wire type 0: varint
//   wire type 2: length-delimited

func encodeVarint(_ value: UInt64, into buf: inout Data) {
    var v = value
    repeat {
        let byte = UInt8(v & 0x7F)
        v >>= 7
        buf.append(v == 0 ? byte : byte | 0x80)
    } while v != 0
}

func encodeVarintField(field: UInt32, value: UInt64, into buf: inout Data) {
    guard value != 0 else { return }
    buf.append(UInt8(field << 3))
    encodeVarint(value, into: &buf)
}

func encodeSInt64Field(field: UInt32, value: Int64, into buf: inout Data) {
    encodeVarintField(field: field, value: UInt64(bitPattern: value), into: &buf)
}

func encodeLengthDelimited(field: UInt32, data: Data, into buf: inout Data) {
    guard !data.isEmpty else { return }
    buf.append(UInt8((field << 3) | 2))
    encodeVarint(UInt64(data.count), into: &buf)
    buf.append(contentsOf: data)
}

func encodeStringField(field: UInt32, string: String, into buf: inout Data) {
    // Always emit even for empty strings — pprof requires string_table[0] == ""
    let bytes = Data(string.utf8)
    buf.append(UInt8((field << 3) | 2))
    encodeVarint(UInt64(bytes.count), into: &buf)
    buf.append(contentsOf: bytes)
}

func encodePackedUInt64(field: UInt32, values: [UInt64], into buf: inout Data) {
    guard !values.isEmpty else { return }
    var inner = Data()
    for v in values { encodeVarint(v, into: &inner) }
    encodeLengthDelimited(field: field, data: inner, into: &buf)
}

func encodePackedInt64(field: UInt32, values: [Int64], into buf: inout Data) {
    guard !values.isEmpty else { return }
    var inner = Data()
    for v in values { encodeVarint(UInt64(bitPattern: v), into: &inner) }
    encodeLengthDelimited(field: field, data: inner, into: &buf)
}
