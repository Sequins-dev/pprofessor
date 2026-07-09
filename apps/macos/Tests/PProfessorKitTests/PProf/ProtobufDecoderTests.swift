import Testing
import Foundation
@testable import PProfessorKit

@Suite("ProtobufDecoder")
struct ProtobufDecoderTests {

    // MARK: - Varint decoding

    @Test func decodeVarintZero() {
        let data = Data([0x00])
        let (value, consumed) = decodeVarint(from: data, at: 0)
        #expect(value == 0)
        #expect(consumed == 1)
    }

    @Test func decodeVarintOne() {
        let data = Data([0x01])
        let (value, consumed) = decodeVarint(from: data, at: 0)
        #expect(value == 1)
        #expect(consumed == 1)
    }

    @Test func decodeVarint127() {
        let data = Data([0x7F])
        let (value, consumed) = decodeVarint(from: data, at: 0)
        #expect(value == 127)
        #expect(consumed == 1)
    }

    @Test func decodeVarint128() {
        // 128 encodes as [0x80, 0x01]
        let data = Data([0x80, 0x01])
        let (value, consumed) = decodeVarint(from: data, at: 0)
        #expect(value == 128)
        #expect(consumed == 2)
    }

    @Test func decodeVarintLarge() {
        // Encode a large value and decode it back
        var buf = Data()
        encodeVarint(300, into: &buf)
        let (value, _) = decodeVarint(from: buf, at: 0)
        #expect(value == 300)
    }

    @Test func decodeVarintRoundTrip() {
        for v: UInt64 in [0, 1, 127, 128, 255, 256, 16383, 16384, UInt64.max / 2] {
            var buf = Data()
            encodeVarint(v, into: &buf)
            let (decoded, _) = decodeVarint(from: buf, at: 0)
            #expect(decoded == v, "Round-trip failed for \(v)")
        }
    }

    // MARK: - Packed array decoding

    @Test func decodePackedUInt64sEmpty() {
        let result = decodePackedUInt64s(from: Data())
        #expect(result.isEmpty)
    }

    @Test func decodePackedUInt64sMultiple() {
        var buf = Data()
        encodeVarint(1, into: &buf)
        encodeVarint(2, into: &buf)
        encodeVarint(300, into: &buf)
        let result = decodePackedUInt64s(from: buf)
        #expect(result == [1, 2, 300])
    }

    @Test func decodePackedInt64sNegative() {
        // Int64(-1) stored as UInt64(bitPattern: -1) = UInt64.max
        var buf = Data()
        encodeVarint(UInt64(bitPattern: -1), into: &buf)
        encodeVarint(UInt64(bitPattern: 42), into: &buf)
        let result = decodePackedInt64s(from: buf)
        #expect(result == [-1, 42])
    }

    // MARK: - decodeProtobuf field dispatch

    @Test func decodeProtobufFieldNumbers() {
        // Build a small message: field 1 (varint) = 42, field 2 (length-delimited) = "hi"
        var buf = Data()
        encodeVarintField(field: 1, value: 42, into: &buf)
        encodeStringField(field: 2, string: "hi", into: &buf)

        var fields: [(UInt32, WireType)] = []
        decodeProtobuf(from: buf) { field, wireType, _ in
            fields.append((field, wireType))
        }

        #expect(fields.count == 2)
        #expect(fields[0].0 == 1 && fields[0].1 == .varint)
        #expect(fields[1].0 == 2 && fields[1].1 == .lengthDelimited)
    }
}
