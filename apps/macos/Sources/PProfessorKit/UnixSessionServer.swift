import Darwin
import Foundation

public final class UnixSessionServer: @unchecked Sendable {
    public typealias Handler = @Sendable (UUID, SessionFrame) -> Void

    private let path: String
    private let handler: Handler
    private let queue = DispatchQueue(label: "com.pprofessor.session-server", qos: .userInitiated, attributes: .concurrent)
    private let lock = NSLock()
    private var listener: Int32 = -1

    public init(path: String, handler: @escaping Handler) {
        self.path = path
        self.handler = handler
    }

    deinit { stop() }

    public func start() throws {
        lock.lock()
        defer { lock.unlock() }
        guard listener < 0 else { return }

        try removeOwnedStaleSocket(at: path)
        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else { throw currentPOSIXError() }
        do {
            var address = try socketAddress(path: path)
            let bindResult = withUnsafePointer(to: &address) {
                $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard bindResult == 0 else { throw currentPOSIXError() }
            guard Darwin.chmod(path, S_IRUSR | S_IWUSR) == 0 else { throw currentPOSIXError() }
            guard Darwin.listen(fd, 16) == 0 else { throw currentPOSIXError() }
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
        lock.unlock()
        if fd >= 0 {
            Darwin.shutdown(fd, SHUT_RDWR)
            Darwin.close(fd)
        }
        try? FileManager.default.removeItem(atPath: path)
    }

    private func acceptLoop(fd: Int32) {
        while true {
            let client = Darwin.accept(fd, nil, nil)
            guard client >= 0 else { return }
            var peerUID: uid_t = 0
            var peerGID: gid_t = 0
            guard getpeereid(client, &peerUID, &peerGID) == 0,
                  isAllowedSessionPeer(peerUID: peerUID, appUID: geteuid()) else {
                Darwin.close(client)
                continue
            }
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

func isAllowedSessionPeer(peerUID: uid_t, appUID: uid_t) -> Bool {
    peerUID == appUID || peerUID == 0
}

private func socketAddress(path: String) throws -> sockaddr_un {
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8) + [0]
    guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else {
        throw POSIXError(.ENAMETOOLONG)
    }
    withUnsafeMutableBytes(of: &address.sun_path) { destination in
        destination.copyBytes(from: bytes)
    }
    return address
}

private func currentPOSIXError() -> POSIXError {
    POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
}

private func removeOwnedStaleSocket(at path: String) throws {
    var status = stat()
    guard Darwin.lstat(path, &status) == 0 else {
        if errno == ENOENT { return }
        throw currentPOSIXError()
    }
    guard status.st_uid == geteuid(), status.st_mode & S_IFMT == S_IFSOCK else {
        throw POSIXError(.EEXIST)
    }
    guard Darwin.unlink(path) == 0 else { throw currentPOSIXError() }
}
