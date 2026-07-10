import Foundation

public enum SessionFrameKind: UInt16, Sendable {
    case hello = 1
    case profileCheckpoint = 2
    case profileDelta = 3
    case finalizing = 4
    case finalProfile = 5
    case failed = 6
    case heartbeat = 7
    case acknowledged = 101
    case stop = 102
    case protocolError = 103
}

public enum SessionStreamError: Error, Equatable {
    case invalidHeader
    case invalidMagic
    case unknownFrameKind(UInt16)
    case payloadTooLarge(UInt64)
}

public struct SessionFrameHeader: Equatable, Sendable {
    public static let encodedLength = 28
    public static let maximumPayloadLength: UInt64 = 256 * 1024 * 1024

    public var major: UInt16
    public var minor: UInt16
    public var kind: SessionFrameKind
    public var flags: UInt16
    public var sequence: UInt64
    public var payloadLength: UInt64

    public init(major: UInt16 = 1, minor: UInt16 = 0, kind: SessionFrameKind, flags: UInt16 = 0, sequence: UInt64 = 0, payloadLength: UInt64 = 0) {
        self.major = major
        self.minor = minor
        self.kind = kind
        self.flags = flags
        self.sequence = sequence
        self.payloadLength = payloadLength
    }

    public init(decoding data: Data) throws {
        guard data.count == Self.encodedLength else { throw SessionStreamError.invalidHeader }
        guard Array(data[0..<4]) == Array("PPRS".utf8) else { throw SessionStreamError.invalidMagic }
        func integer<T: FixedWidthInteger>(_ range: Range<Int>, as: T.Type) -> T {
            data[range].reduce(0) { ($0 << 8) | T($1) }
        }
        major = integer(4..<6, as: UInt16.self)
        minor = integer(6..<8, as: UInt16.self)
        let rawKind = integer(8..<10, as: UInt16.self)
        guard let kind = SessionFrameKind(rawValue: rawKind) else { throw SessionStreamError.unknownFrameKind(rawKind) }
        self.kind = kind
        flags = integer(10..<12, as: UInt16.self)
        sequence = integer(12..<20, as: UInt64.self)
        payloadLength = integer(20..<28, as: UInt64.self)
        guard payloadLength <= Self.maximumPayloadLength else { throw SessionStreamError.payloadTooLarge(payloadLength) }
    }

    public func encoded() -> Data {
        var data = Data("PPRS".utf8)
        func append<T: FixedWidthInteger>(_ value: T) {
            withUnsafeBytes(of: value.bigEndian) { data.append(contentsOf: $0) }
        }
        append(major)
        append(minor)
        append(kind.rawValue)
        append(flags)
        append(sequence)
        append(payloadLength)
        return data
    }
}

public struct SessionFrame: Equatable, Sendable {
    public let header: SessionFrameHeader
    public let payload: Data
}

public struct SessionFrameParser: Sendable {
    private var buffer = Data()

    public init() {}

    public mutating func append<S: DataProtocol>(_ bytes: S) throws -> [SessionFrame] {
        buffer.append(contentsOf: bytes)
        var frames: [SessionFrame] = []
        while buffer.count >= SessionFrameHeader.encodedLength {
            let headerData = Data(buffer.prefix(SessionFrameHeader.encodedLength))
            let header = try SessionFrameHeader(decoding: headerData)
            let total = SessionFrameHeader.encodedLength + Int(header.payloadLength)
            guard buffer.count >= total else { break }
            let payloadStart = buffer.index(
                buffer.startIndex,
                offsetBy: SessionFrameHeader.encodedLength
            )
            let payloadEnd = buffer.index(buffer.startIndex, offsetBy: total)
            let payload = Data(buffer[payloadStart..<payloadEnd])
            frames.append(SessionFrame(header: header, payload: payload))
            buffer.removeFirst(total)
        }
        return frames
    }
}
