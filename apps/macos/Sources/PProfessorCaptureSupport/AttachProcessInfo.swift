import Foundation

public struct AttachProcessInfo: Codable, Identifiable, Hashable, Sendable {
    public let pid: UInt32
    public let parentPid: UInt32
    public let uid: UInt32
    public let name: String
    public let executablePath: String?
    public let startTimeMicros: UInt64
    public let architecture: String
    public let attachable: Bool?
    public let attachabilityReason: String?

    public var id: String { "\(pid)-\(startTimeMicros)" }
    public var canAttach: Bool { attachable != false }

    public static func pickerTargets(
        _ processes: [AttachProcessInfo],
        excludingPID: UInt32
    ) -> [AttachProcessInfo] {
        processes.filter { $0.pid != excludingPID && $0.canAttach }
    }
}
