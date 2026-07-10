import AppKit
import Foundation
import SwiftUI
import PProfessorKit

@MainActor
@Observable
final class ProfileViewModel {
    // MARK: - Observable state

    private(set) var nodes: [FlamegraphNode] = []
    private(set) var nodeIndex: [String: Int] = [:]
    private(set) var rootNodeId: String?
    private(set) var totalValue: Int64 = 0
    private(set) var isLoading = false
    private(set) var error: String?
    private(set) var availableValueTypes: [(type: String, unit: String)] = []
    private(set) var availableThreads: [String] = []
    private(set) var sourceURL: URL?

    // MARK: - UI state

    var searchText: String = ""
    var selectedValueTypeIndex: Int = 0 {
        didSet {
            guard let profile = decodedProfile else { return }
            recompute(profile: profile)
        }
    }
    var selectedThread: String? = nil {
        didSet {
            guard let profile = decodedProfile else { return }
            recompute(profile: profile)
        }
    }
    var zoomedNodeId: String?

    // MARK: - Private

    private var decodedProfile: DecodedProfile?

    func loadDecodedProfile(_ profile: DecodedProfile) {
        error = nil
        isLoading = false
        decodedProfile = profile
        sourceURL = nil
        availableValueTypes = profile.sampleTypes.map { vt in
            (type: profile.string(at: vt.type), unit: profile.string(at: vt.unit))
        }
        if !availableValueTypes.indices.contains(selectedValueTypeIndex) {
            selectedValueTypeIndex = 0
        }
        recompute(profile: profile)
    }

    // MARK: - File loading

    func loadFile(url: URL) async {
        isLoading = true
        error = nil
        nodes = []
        nodeIndex = [:]
        rootNodeId = nil
        decodedProfile = nil
        availableValueTypes = []

        do {
            let rawData = try await Task.detached(priority: .userInitiated) {
                try Data(contentsOf: url)
            }.value

            let uncompressed = try await Task.detached(priority: .userInitiated) {
                // Gzip magic bytes: 0x1f 0x8b
                let isGzip = rawData.count >= 2 && rawData[rawData.startIndex] == 0x1f && rawData[rawData.startIndex + 1] == 0x8b
                return isGzip ? try decompressGzip(data: rawData) : rawData
            }.value

            let profile = await Task.detached(priority: .userInitiated) {
                DecodedProfile.decode(from: uncompressed)
            }.value

            decodedProfile = profile
            sourceURL = url
            availableValueTypes = profile.sampleTypes.map { vt in
                (type: profile.string(at: vt.type), unit: profile.string(at: vt.unit))
            }
            selectedValueTypeIndex = 0
            selectedThread = nil
            recompute(profile: profile)
        } catch {
            self.error = error.localizedDescription
        }

        isLoading = false
    }

    func saveProfile() {
        guard let sourceURL else { return }
        let panel = NSSavePanel()
        panel.title = "Save Profile"
        panel.nameFieldStringValue = sourceURL.lastPathComponent
        panel.canCreateDirectories = true
        guard panel.runModal() == .OK, let destination = panel.url else { return }
        do {
            if FileManager.default.fileExists(atPath: destination.path) {
                try FileManager.default.removeItem(at: destination)
            }
            try FileManager.default.copyItem(at: sourceURL, to: destination)
        } catch {
            self.error = error.localizedDescription
        }
    }

    // MARK: - Node lookup

    func getNode(nodeId: String) -> FlamegraphNode? {
        guard let idx = nodeIndex[nodeId], idx < nodes.count else { return nil }
        return nodes[idx]
    }

    func getStackTrace(for nodeId: String) -> [FlamegraphNode] {
        var result: [FlamegraphNode] = []
        var currentId: String? = nodeId
        while let id = currentId, let node = getNode(nodeId: id) {
            result.insert(node, at: 0)
            currentId = node.parentId
        }
        return result
    }

    // MARK: - Export

    func exportAsJSON() {
        let panel = NSSavePanel()
        panel.title = "Export Profile"
        panel.nameFieldStringValue = "profile-export.json"
        panel.allowedContentTypes = [.json]
        panel.canCreateDirectories = true

        guard panel.runModal() == .OK, let url = panel.url else { return }

        let jsonArray: [[String: Any]] = nodes.map { node in
            var dict: [String: Any] = [
                "id": node.id,
                "functionName": node.functionName,
                "depth": node.depth,
                "selfValue": node.selfValue,
                "totalValue": node.totalValue,
                "selfPercentage": node.selfPercentage,
                "totalPercentage": node.totalPercentage,
                "childIds": node.childIds,
            ]
            if let filename = node.filename { dict["filename"] = filename }
            if let line = node.line { dict["line"] = line }
            if let parentId = node.parentId { dict["parentId"] = parentId }
            if let systemName = node.systemName { dict["systemName"] = systemName }
            return dict
        }

        do {
            let data = try JSONSerialization.data(withJSONObject: jsonArray, options: [.prettyPrinted, .sortedKeys])
            try data.write(to: url)
        } catch {
            NSLog("Failed to export profile: \(error)")
        }
    }

    // MARK: - Private

    private func recompute(profile: DecodedProfile) {
        let result = convertProfile(profile, valueTypeIndex: selectedValueTypeIndex, threadFilter: selectedThread)
        nodes = result.nodes
        nodeIndex = result.nodeIndex
        rootNodeId = result.rootNodeId
        totalValue = result.totalValue
        availableThreads = result.availableThreads
    }
}
