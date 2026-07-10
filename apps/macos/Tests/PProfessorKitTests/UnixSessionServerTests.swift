import Darwin
import Foundation
import Testing
@testable import PProfessorKit

@Suite("Unix session server")
struct UnixSessionServerTests {
    actor Received {
        var frames: [SessionFrame] = []
        func append(_ frame: SessionFrame) { frames.append(frame) }
        var count: Int { frames.count }
    }

    @Test func receivesAFramedMessage() async throws {
        let path = "/tmp/pprofessor-swift-\(getpid())-\(UInt32.random(in: 0...UInt32.max)).sock"
        let received = Received()
        let server = UnixSessionServer(path: path) { _, frame in
            Task { await received.append(frame) }
        }
        try server.start()
        defer { server.stop() }

        let client = try connectUnixSocket(path: path)
        defer { Darwin.close(client) }
        let payload = Data([4, 5, 6])
        let header = SessionFrameHeader(kind: .profileDelta, sequence: 8, payloadLength: UInt64(payload.count))
        let bytes = header.encoded() + payload
        _ = bytes.withUnsafeBytes { Darwin.write(client, $0.baseAddress, $0.count) }

        for _ in 0..<50 where await received.count == 0 {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(await received.count == 1)
    }

    @Test func refusesToReplaceARegularFileAtTheSocketPath() throws {
        let path = "/tmp/pprofessor-swift-regular-\(getpid())-\(UInt32.random(in: 0...UInt32.max))"
        let contents = Data("keep me".utf8)
        try contents.write(to: URL(fileURLWithPath: path))
        defer { try? FileManager.default.removeItem(atPath: path) }

        let server = UnixSessionServer(path: path) { _, _ in }
        #expect(throws: POSIXError.self) {
            try server.start()
        }
        #expect(try Data(contentsOf: URL(fileURLWithPath: path)) == contents)
    }

    @Test func acceptsOnlyTheAppUserOrRootAsPeers() {
        #expect(isAllowedSessionPeer(peerUID: 501, appUID: 501))
        #expect(isAllowedSessionPeer(peerUID: 0, appUID: 501))
        #expect(!isAllowedSessionPeer(peerUID: 502, appUID: 501))
    }
}

private func connectUnixSocket(path: String) throws -> Int32 {
    let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { throw POSIXError(.ENOTSOCK) }
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8) + [0]
    guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { throw POSIXError(.ENAMETOOLONG) }
    withUnsafeMutableBytes(of: &address.sun_path) { destination in
        destination.copyBytes(from: bytes)
    }
    let result = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard result == 0 else {
        Darwin.close(fd)
        throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .ECONNREFUSED)
    }
    return fd
}
