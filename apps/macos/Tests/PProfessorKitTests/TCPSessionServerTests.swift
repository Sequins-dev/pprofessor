import Darwin
import Foundation
import Testing
@testable import PProfessorKit

@Suite("TCP session server")
struct TCPSessionServerTests {
    actor Received {
        var frames: [SessionFrame] = []
        func append(_ frame: SessionFrame) { frames.append(frame) }
        var count: Int { frames.count }
    }

    @Test func receivesAFramedMessage() async throws {
        let received = Received()
        let server = TCPSessionServer(port: 0) { _, frame in
            Task { await received.append(frame) }
        }
        try server.start()
        defer { server.stop() }

        let client = try connectLoopbackSocket(port: server.boundPort)
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

    @Test func bindsOnlyToIPv4Loopback() throws {
        let server = TCPSessionServer(port: 0) { _, _ in }
        try server.start()
        defer { server.stop() }

        #expect(server.boundAddress == "127.0.0.1")
        #expect(server.boundPort != 0)
    }
}

private func connectLoopbackSocket(port: UInt16) throws -> Int32 {
    let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
    guard fd >= 0 else { throw POSIXError(.ENOTSOCK) }
    var address = sockaddr_in()
    address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
    address.sin_family = sa_family_t(AF_INET)
    address.sin_port = port.bigEndian
    address.sin_addr = in_addr(s_addr: inet_addr("127.0.0.1"))
    let result = withUnsafePointer(to: &address) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
        }
    }
    guard result == 0 else {
        Darwin.close(fd)
        throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .ECONNREFUSED)
    }
    return fd
}
