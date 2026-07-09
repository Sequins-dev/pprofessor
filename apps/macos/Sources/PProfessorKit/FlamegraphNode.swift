import Foundation

/// A single node in the flamegraph tree, ready for rendering.
public struct FlamegraphNode: Identifiable, Equatable, Sendable {
    /// Path key from root: "func0/func1/.../funcN" (or "root" for the synthetic root).
    public let id: String
    public let functionName: String
    public let systemName: String?
    public let filename: String?
    public let line: Int64?
    public let depth: Int
    public let selfValue: Int64
    public let totalValue: Int64
    public let parentId: String?
    public let childIds: [String]
    public let selfPercentage: Double
    public let totalPercentage: Double

    public init(
        id: String,
        functionName: String,
        systemName: String?,
        filename: String?,
        line: Int64?,
        depth: Int,
        selfValue: Int64,
        totalValue: Int64,
        parentId: String?,
        childIds: [String],
        selfPercentage: Double,
        totalPercentage: Double
    ) {
        self.id = id
        self.functionName = functionName
        self.systemName = systemName
        self.filename = filename
        self.line = line
        self.depth = depth
        self.selfValue = selfValue
        self.totalValue = totalValue
        self.parentId = parentId
        self.childIds = childIds
        self.selfPercentage = selfPercentage
        self.totalPercentage = totalPercentage
    }
}
