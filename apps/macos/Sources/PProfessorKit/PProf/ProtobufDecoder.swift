import Foundation

// Protobuf wire types used by pprof
enum WireType: UInt8 {
    case varint = 0
    case lengthDelimited = 2
}

/// Decode a varint from `data` starting at `offset`.
/// Returns the decoded value and the number of bytes consumed.
func decodeVarint(from data: Data, at offset: Int) -> (UInt64, Int) {
    var result: UInt64 = 0
    var shift: UInt64 = 0
    var i = offset
    while i < data.count {
        let byte = data[data.startIndex + i]
        result |= UInt64(byte & 0x7F) << shift
        i += 1
        if byte & 0x80 == 0 { break }
        shift += 7
    }
    return (result, i - offset)
}

/// Decode a packed repeated uint64 field (length-delimited buffer of varints).
func decodePackedUInt64s(from data: Data) -> [UInt64] {
    var result: [UInt64] = []
    var i = 0
    while i < data.count {
        let (value, consumed) = decodeVarint(from: data, at: i)
        result.append(value)
        i += consumed
    }
    return result
}

/// Decode a packed repeated int64 field (length-delimited buffer of varints).
func decodePackedInt64s(from data: Data) -> [Int64] {
    return decodePackedUInt64s(from: data).map { Int64(bitPattern: $0) }
}

/// Visit every field in a protobuf message. The visitor receives:
/// - fieldNumber: the protobuf field number
/// - wireType: .varint or .lengthDelimited
/// - value: the raw bytes for that field (varint bytes or length-prefixed content)
func decodeProtobuf(from data: Data, visitor: (UInt32, WireType, Data) -> Void) {
    var i = 0
    while i < data.count {
        let (tag, tagBytes) = decodeVarint(from: data, at: i)
        i += tagBytes
        let fieldNumber = UInt32(tag >> 3)
        let rawWireType = UInt8(tag & 0x7)

        switch rawWireType {
        case 0: // varint
            var end = i
            while end < data.count {
                let byte = data[data.startIndex + end]
                end += 1
                if byte & 0x80 == 0 { break }
            }
            let fieldData = data[data.startIndex + i ..< data.startIndex + end]
            visitor(fieldNumber, .varint, fieldData)
            i = end

        case 2: // length-delimited
            let (length, lengthBytes) = decodeVarint(from: data, at: i)
            i += lengthBytes
            let end = i + Int(length)
            guard end <= data.count else { return }
            let fieldData = data[data.startIndex + i ..< data.startIndex + end]
            visitor(fieldNumber, .lengthDelimited, fieldData)
            i = end

        default:
            // Unknown wire type — cannot safely skip, stop parsing
            return
        }
    }
}
