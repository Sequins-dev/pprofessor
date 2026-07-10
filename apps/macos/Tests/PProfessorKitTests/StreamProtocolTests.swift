import Foundation
import Testing
@testable import PProfessorKit

@Suite("Session stream protocol")
struct StreamProtocolTests {
    @Test func headerRoundTrips() throws {
        let header = SessionFrameHeader(
            kind: .profileDelta,
            flags: 3,
            sequence: 42,
            payloadLength: 65_536
        )
        let encoded = header.encoded()
        #expect(encoded.count == SessionFrameHeader.encodedLength)
        #expect(Array(encoded.prefix(4)) == Array("PPRS".utf8))
        #expect(try SessionFrameHeader(decoding: encoded) == header)
    }

    @Test func parserWaitsForACompleteFrame() throws {
        let payload = Data([1, 2, 3, 4])
        let header = SessionFrameHeader(kind: .profileDelta, sequence: 7, payloadLength: UInt64(payload.count))
        let bytes = header.encoded() + payload
        var parser = SessionFrameParser()

        #expect(try parser.append(bytes.prefix(5)).isEmpty)
        let frames = try parser.append(bytes.dropFirst(5))
        #expect(frames.count == 1)
        #expect(frames[0].header.sequence == 7)
        #expect(frames[0].payload == payload)
    }

    @Test func parserHandlesMultipleFramesInOneRead() throws {
        let helloPayload = Data(repeating: 0x48, count: 139)
        let deltaPayload = Data(repeating: 0x44, count: 64)
        let hello = SessionFrameHeader(
            kind: .hello,
            sequence: 1,
            payloadLength: UInt64(helloPayload.count)
        ).encoded() + helloPayload
        let delta = SessionFrameHeader(
            kind: .profileDelta,
            sequence: 2,
            payloadLength: UInt64(deltaPayload.count)
        ).encoded() + deltaPayload
        var parser = SessionFrameParser()

        let frames = try parser.append(hello + delta)

        #expect(frames.map(\.header.kind) == [.hello, .profileDelta])
        #expect(frames.map(\.payload) == [helloPayload, deltaPayload])
    }
}
