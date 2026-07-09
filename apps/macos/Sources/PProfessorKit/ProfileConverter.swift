import Foundation

/// Result of converting a decoded pprof profile into a renderable flamegraph tree.
public struct ProfileConversionResult: Sendable {
    public let nodes: [FlamegraphNode]
    public let nodeIndex: [String: Int]
    public let rootNodeId: String?
    public let totalValue: Int64
    public let valueTypeNames: [(type: String, unit: String)]
    public let availableThreads: [String]
}

/// Convert a decoded pprof profile into a flamegraph tree.
///
/// - Parameters:
///   - profile: The decoded pprof profile.
///   - valueTypeIndex: Which sample value column to use (default: 0).
///   - threadFilter: Thread name to filter to, or nil for all threads.
/// - Returns: A flat array of `FlamegraphNode` values in BFS order with a lookup index.
public func convertProfile(
    _ profile: DecodedProfile,
    valueTypeIndex: Int = 0,
    threadFilter: String? = nil
) -> ProfileConversionResult {
    // Build value type names for the UI selector
    let valueTypeNames: [(type: String, unit: String)] = profile.sampleTypes.map { vt in
        (type: profile.string(at: vt.type), unit: profile.string(at: vt.unit))
    }

    // Keys that indicate thread identity in pprof labels
    let threadKeys: Set<String> = ["thread", "tid", "thread_id", "thread_name", "threadName", "thread.id", "thread.name"]

    /// Extract the thread label string from a sample, if any.
    func threadLabel(for sample: ProfSample) -> String? {
        for label in sample.labels {
            let key = profile.string(at: label.key)
            guard threadKeys.contains(key) else { continue }
            if label.str != 0 {
                return profile.string(at: label.str)
            } else if label.num != 0 {
                return "\(label.num)"
            }
        }
        return nil
    }

    // Collect all unique thread labels across all samples (sorted)
    var threadSet = Set<String>()
    for sample in profile.samples {
        if let t = threadLabel(for: sample) { threadSet.insert(t) }
    }
    let availableThreads = threadSet.sorted()

    guard !profile.samples.isEmpty else {
        return ProfileConversionResult(
            nodes: [],
            nodeIndex: [:],
            rootNodeId: nil,
            totalValue: 0,
            valueTypeNames: valueTypeNames,
            availableThreads: availableThreads
        )
    }

    // Build lookup tables
    let functionsByID: [UInt64: ProfFunction] = Dictionary(
        profile.functions.map { ($0.id, $0) },
        uniquingKeysWith: { first, _ in first }
    )
    let locationsByID: [UInt64: ProfLocation] = Dictionary(
        profile.locations.map { ($0.id, $0) },
        uniquingKeysWith: { first, _ in first }
    )

    // Trie node for tree construction
    final class TrieNode {
        let id: String
        let functionName: String
        let systemName: String?
        let filename: String?
        let line: Int64?
        let depth: Int
        let parentId: String?
        var selfValue: Int64 = 0
        var totalValue: Int64 = 0
        var children: [String: TrieNode] = [:]
        var childOrder: [String] = []

        init(
            id: String, functionName: String, systemName: String?,
            filename: String?, line: Int64?, depth: Int, parentId: String?
        ) {
            self.id = id
            self.functionName = functionName
            self.systemName = systemName
            self.filename = filename
            self.line = line
            self.depth = depth
            self.parentId = parentId
        }
    }

    let root = TrieNode(
        id: "root", functionName: "root", systemName: nil,
        filename: nil, line: nil, depth: 0, parentId: nil
    )

    for sample in profile.samples {
        guard valueTypeIndex < sample.values.count else { continue }
        let value = sample.values[valueTypeIndex]
        guard value > 0 else { continue }

        // Apply thread filter
        if let filter = threadFilter {
            guard threadLabel(for: sample) == filter else { continue }
        }

        // pprof location IDs are leaf-first; reverse to get root→leaf order
        let locationIDs = sample.locationIDs.reversed()

        var frames: [(name: String, systemName: String?, filename: String?, line: Int64?)] = []
        for locID in locationIDs {
            guard let location = locationsByID[locID] else { continue }
            for profLine in location.lines {
                guard let fn = functionsByID[profLine.functionID] else { continue }
                let name = profile.string(at: fn.name)
                let sysName = fn.systemName != 0 ? profile.string(at: fn.systemName) : nil
                let file = fn.filename != 0 ? profile.string(at: fn.filename) : nil
                let line: Int64? = profLine.line != 0 ? profLine.line : nil
                frames.append((name: name, systemName: sysName, filename: file, line: line))
            }
        }

        var current = root
        root.totalValue += value
        var pathKey = "root"

        for (i, frame) in frames.enumerated() {
            let childPathKey = pathKey + "/" + frame.name
            if let existing = current.children[childPathKey] {
                existing.totalValue += value
                current = existing
            } else {
                let child = TrieNode(
                    id: childPathKey,
                    functionName: frame.name,
                    systemName: frame.systemName,
                    filename: frame.filename,
                    line: frame.line,
                    depth: i + 1,
                    parentId: current.id
                )
                child.totalValue += value
                current.children[childPathKey] = child
                current.childOrder.append(childPathKey)
                current = child
            }
            pathKey = childPathKey
        }

        current.selfValue += value
    }

    let grandTotal = root.totalValue

    var nodes: [FlamegraphNode] = []
    var nodeIndex: [String: Int] = [:]
    var queue: [TrieNode] = [root]

    while !queue.isEmpty {
        let node = queue.removeFirst()
        let selfPct = grandTotal > 0 ? Double(node.selfValue) / Double(grandTotal) * 100 : 0
        let totalPct = grandTotal > 0 ? Double(node.totalValue) / Double(grandTotal) * 100 : 0

        let flamegraphNode = FlamegraphNode(
            id: node.id,
            functionName: node.functionName,
            systemName: node.systemName,
            filename: node.filename,
            line: node.line,
            depth: node.depth,
            selfValue: node.selfValue,
            totalValue: node.totalValue,
            parentId: node.parentId,
            childIds: node.childOrder,
            selfPercentage: selfPct,
            totalPercentage: totalPct
        )

        nodeIndex[node.id] = nodes.count
        nodes.append(flamegraphNode)

        for childId in node.childOrder {
            if let child = node.children[childId] {
                queue.append(child)
            }
        }
    }

    return ProfileConversionResult(
        nodes: nodes,
        nodeIndex: nodeIndex,
        rootNodeId: grandTotal > 0 ? "root" : nil,
        totalValue: grandTotal,
        valueTypeNames: valueTypeNames,
        availableThreads: availableThreads
    )
}
