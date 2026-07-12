import Darwin
import Foundation

public final class TCPSessionServer: @unchecked Sendable {
    public static let defaultPort: UInt16 = 57_557
    public typealias Handler = @Sendable (UUID, SessionFrame) -> Void

    public let boundAddress = "127.0.0.1"
    public private(set) var boundPort: UInt16 = 0

    private let requestedPort: UInt16
    private let handler: Handler
    private let queue = DispatchQueue(
        label: "dev.sequins.pprofessor.session-server",
        qos: .userInitiated,
        attributes: .concurrent
    )
    private let lock = NSLock()
    private var listener: Int32 = -1

    public init(port: UInt16 = TCPSessionServer.defaultPort, handler: @escaping Handler) {
        requestedPort = port
        self.handler = handler
    }

    deinit { stop() }

    public func start() throws {
        lock.lock()
        defer { lock.unlock() }
        guard listener < 0 else { return }

        let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw currentPOSIXError() }
        do {
            var reuse: Int32 = 1
            guard Darwin.setsockopt(
                fd,
                SOL_SOCKET,
                SO_REUSEADDR,
                &reuse,
                socklen_t(MemoryLayout<Int32>.size)
            ) == 0 else { throw currentPOSIXError() }

            var address = sockaddr_in()
            address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
            address.sin_family = sa_family_t(AF_INET)
            address.sin_port = requestedPort.bigEndian
            address.sin_addr = in_addr(s_addr: inet_addr(boundAddress))
            let bindResult = withUnsafePointer(to: &address) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
                }
            }
            guard bindResult == 0 else { throw currentPOSIXError() }
            guard Darwin.listen(fd, 16) == 0 else { throw currentPOSIXError() }

            var localAddress = sockaddr_in()
            var localAddressLength = socklen_t(MemoryLayout<sockaddr_in>.size)
            let nameResult = withUnsafeMutablePointer(to: &localAddress) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.getsockname(fd, $0, &localAddressLength)
                }
            }
            guard nameResult == 0 else { throw currentPOSIXError() }
            boundPort = UInt16(bigEndian: localAddress.sin_port)
            listener = fd
        } catch {
            Darwin.close(fd)
            throw error
        }

        queue.async { [weak self] in self?.acceptLoop(fd: fd) }
    }

    public func stop() {
        lock.lock()
        let fd = listener
        listener = -1
        boundPort = 0
        lock.unlock()
        if fd >= 0 {
            Darwin.shutdown(fd, SHUT_RDWR)
            Darwin.close(fd)
        }
    }

    private func acceptLoop(fd: Int32) {
        while true {
            let client = Darwin.accept(fd, nil, nil)
            guard client >= 0 else { return }
            let connectionID = UUID()
            queue.async { [weak self] in self?.readLoop(fd: client, connectionID: connectionID) }
        }
    }

    private func readLoop(fd: Int32, connectionID: UUID) {
        defer { Darwin.close(fd) }
        var parser = SessionFrameParser()
        var bytes = [UInt8](repeating: 0, count: 64 * 1024)
        while true {
            let count = Darwin.read(fd, &bytes, bytes.count)
            guard count > 0 else { return }
            do {
                for frame in try parser.append(bytes.prefix(count)) {
                    handler(connectionID, frame)
                }
            } catch {
                return
            }
        }
    }
}

private func currentPOSIXError() -> POSIXError {
    POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
}
