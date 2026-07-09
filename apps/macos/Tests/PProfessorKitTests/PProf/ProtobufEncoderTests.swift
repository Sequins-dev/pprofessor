import Foundation
import Testing
@testable import PProfessorKit

@Suite("ProtobufEncoder")
struct ProtobufEncoderTests {
    @Test func varintZero() {
        var buf = Data()
        encodeVarint(0, into: &buf)
        #expect(buf == Data([0x00]))
    }

    @Test func varintOne() {
        var buf = Data()
        encodeVarint(1, into: &buf)
        #expect(buf == Data([0x01]))
    }

    @Test func varint127() {
        var buf = Data()
        encodeVarint(127, into: &buf)
        #expect(buf == Data([0x7F]))
    }

    @Test func varint128() {
        var buf = Data()
        encodeVarint(128, into: &buf)
        #expect(buf == Data([0x80, 0x01]))
    }

    @Test func varintFieldSkipsZero() {
        var buf = Data()
        encodeVarintField(field: 1, value: 0, into: &buf)
        #expect(buf.isEmpty)
    }

    @Test func lengthDelimitedSkipsEmpty() {
        var buf = Data()
        encodeLengthDelimited(field: 1, data: Data(), into: &buf)
        #expect(buf.isEmpty)
    }

    @Test func stringFieldEmitsForEmpty() {
        var buf = Data()
        encodeStringField(field: 6, string: "", into: &buf)
        #expect(buf == Data([0x32, 0x00]))
    }

    @Test func packedUInt64HasCorrectTag() {
        var buf = Data()
        encodePackedUInt64(field: 1, values: [1, 2, 3], into: &buf)
        #expect(!buf.isEmpty)
        #expect(buf[0] == 0x0a)
    }
}
