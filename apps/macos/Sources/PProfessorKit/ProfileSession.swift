import Foundation
import SwiftData

public enum ProfileSessionSource: String, Codable, Sendable {
    case appAttach
    case cliAttach
    case cliRun
    case imported
}

public enum ProfileSessionStatus: String, Codable, Sendable {
    case connecting
    case live
    case finalizing
    case completed
    case failed
    case interrupted
}

@Model
public final class ProfileSession {
    @Attribute(.unique) public var id: UUID
    public var displayName: String
    public var sourceRaw: String
    public var statusRaw: String
    public var pid: Int?
    public var processName: String?
    public var command: String?
    public var architecture: String?
    public var frequencyHz: Int
    public var startedAt: Date
    public var endedAt: Date?
    public var durationNanos: Int64
    public var sampleCount: Int64
    public var artifactRelativePath: String?
    public var journalRelativePath: String?
    public var errorSummary: String?
    public var createdAt: Date
    public var updatedAt: Date

    public var source: ProfileSessionSource {
        get { ProfileSessionSource(rawValue: sourceRaw) ?? .imported }
        set { sourceRaw = newValue.rawValue }
    }

    public var status: ProfileSessionStatus {
        get { ProfileSessionStatus(rawValue: statusRaw) ?? .failed }
        set { statusRaw = newValue.rawValue }
    }

    public init(
        id: UUID = UUID(),
        displayName: String,
        source: ProfileSessionSource,
        status: ProfileSessionStatus,
        pid: Int? = nil,
        processName: String? = nil,
        command: String? = nil,
        architecture: String? = nil,
        frequencyHz: Int,
        startedAt: Date = Date()
    ) {
        self.id = id
        self.displayName = displayName
        self.sourceRaw = source.rawValue
        self.statusRaw = status.rawValue
        self.pid = pid
        self.processName = processName
        self.command = command
        self.architecture = architecture
        self.frequencyHz = frequencyHz
        self.startedAt = startedAt
        self.durationNanos = 0
        self.sampleCount = 0
        self.createdAt = Date()
        self.updatedAt = Date()
    }
}

public enum PProfessorSchemaV1: VersionedSchema {
    public static let versionIdentifier = Schema.Version(1, 0, 0)
    public static var models: [any PersistentModel.Type] { [ProfileSession.self] }
}
