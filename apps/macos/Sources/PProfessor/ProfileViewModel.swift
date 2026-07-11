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
    private(set) var timeline: ProfileTimeline?
    private(set) var timelineSelection: TimelineSelectionState?

    // MARK: - UI state

    var searchText: String = ""
    var selectedValueTypeIndex: Int = 0 {
        didSet {
            guard let profile = decodedProfile else { return }
            invalidateLivePresentation()
            rebuildTimeline(profile: profile)
            recompute(profile: profile)
        }
    }
    var selectedThread: String? = nil {
        didSet {
            guard let profile = decodedProfile else { return }
            invalidateLivePresentation()
            rebuildTimeline(profile: profile)
            recompute(profile: profile)
        }
    }
    var zoomedNodeId: String?

    // MARK: - Private

    private var decodedProfile: DecodedProfile?
    private var rangeThrottleTask: Task<Void, Never>?
    private var rangeConversionTask: Task<Void, Never>?
    private var pendingRangeProfile: DecodedProfile?
    private var recomputeGeneration = 0
    private struct PendingLivePresentation {
        let profile: DecodedProfile
        let resetSelection: Bool
    }
    private var pendingLivePresentation: PendingLivePresentation?
    private var livePresentationTask: Task<Void, Never>?
    private var livePresentationGeneration = 0

    func loadDecodedProfile(_ profile: DecodedProfile, resetTimelineSelection: Bool = false) {
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
        enqueueLivePresentation(profile: profile, resetSelection: resetTimelineSelection)
    }

    func selectTimeRange(_ range: ClosedRange<Int64>, isDragging: Bool) {
        guard var selection = timelineSelection, let profile = decodedProfile else { return }
        selection.select(range)
        timelineSelection = selection
        scheduleRecompute(profile: profile, delayNanos: isDragging ? 100_000_000 : 0)
    }

    func resetTimeRange() {
        guard var selection = timelineSelection,
              let duration = timeline?.durationNanos,
              let profile = decodedProfile
        else { return }
        selection.reset(durationNanos: duration)
        timelineSelection = selection
        scheduleRecompute(profile: profile, delayNanos: 0)
    }

    // MARK: - File loading

    func loadFile(url: URL) async {
        invalidateLivePresentation()
        isLoading = true
        error = nil
        nodes = []
        nodeIndex = [:]
        rootNodeId = nil
        decodedProfile = nil
        availableValueTypes = []
        timeline = nil
        timelineSelection = nil

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
            rebuildTimeline(profile: profile, resetSelection: true)
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
        let result = convertProfile(
            profile,
            valueTypeIndex: selectedValueTypeIndex,
            threadFilter: selectedThread,
            timeRangeNanos: timelineSelection?.range
        )
        apply(result)
    }

    private func enqueueLivePresentation(profile: DecodedProfile, resetSelection: Bool) {
        livePresentationGeneration += 1
        pendingLivePresentation = PendingLivePresentation(
            profile: profile,
            resetSelection: resetSelection
        )
        startNextLivePresentationIfNeeded()
    }

    private func startNextLivePresentationIfNeeded() {
        guard livePresentationTask == nil, let request = pendingLivePresentation else { return }
        pendingLivePresentation = nil
        let generation = livePresentationGeneration
        let valueTypeIndex = selectedValueTypeIndex
        let threadFilter = selectedThread
        let selection = timelineSelection
        livePresentationTask = Task { [weak self] in
            let presentation = await Task.detached(priority: .userInitiated) {
                buildProfilePresentation(
                    profile: request.profile,
                    valueTypeIndex: valueTypeIndex,
                    threadFilter: threadFilter,
                    selection: selection,
                    resetSelection: request.resetSelection,
                    bucketCount: 512
                )
            }.value
            guard let self else { return }
            let shouldApply = !Task.isCancelled
                && self.livePresentationGeneration == generation
                && self.pendingLivePresentation == nil
            self.livePresentationTask = nil
            if shouldApply {
                self.timeline = presentation.timeline
                self.timelineSelection = presentation.timelineSelection
                self.apply(presentation.conversion)
            }
            self.startNextLivePresentationIfNeeded()
        }
    }

    private func invalidateLivePresentation() {
        livePresentationGeneration += 1
        pendingLivePresentation = nil
        livePresentationTask?.cancel()
    }

    private func rebuildTimeline(profile: DecodedProfile, resetSelection: Bool = false) {
        timeline = ProfileTimeline.build(
            from: profile,
            valueTypeIndex: selectedValueTypeIndex,
            threadFilter: selectedThread,
            bucketCount: 512
        )
        guard let timeline else {
            timelineSelection = nil
            return
        }
        if resetSelection || timelineSelection == nil {
            timelineSelection = TimelineSelectionState(durationNanos: timeline.durationNanos)
        } else {
            timelineSelection?.updateDuration(timeline.durationNanos)
        }
    }

    private func scheduleRecompute(profile: DecodedProfile, delayNanos: UInt64) {
        recomputeGeneration += 1
        pendingRangeProfile = profile
        if delayNanos == 0 {
            rangeThrottleTask?.cancel()
            rangeThrottleTask = nil
            launchRangeRecompute(profile: profile, generation: recomputeGeneration)
            return
        }
        guard rangeThrottleTask == nil else { return }
        rangeThrottleTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: delayNanos)
            guard !Task.isCancelled, let self, let latestProfile = self.pendingRangeProfile else { return }
            self.rangeThrottleTask = nil
            self.launchRangeRecompute(profile: latestProfile, generation: self.recomputeGeneration)
        }
    }

    private func launchRangeRecompute(profile: DecodedProfile, generation: Int) {
        rangeConversionTask?.cancel()
        let valueTypeIndex = selectedValueTypeIndex
        let threadFilter = selectedThread
        let timeRange = timelineSelection?.range

        rangeConversionTask = Task { [weak self] in
            let result = await Task.detached(priority: .userInitiated) {
                convertProfile(
                    profile,
                    valueTypeIndex: valueTypeIndex,
                    threadFilter: threadFilter,
                    timeRangeNanos: timeRange
                )
            }.value
            guard !Task.isCancelled, self?.recomputeGeneration == generation else { return }
            self?.apply(result)
        }
    }

    private func apply(_ result: ProfileConversionResult) {
        nodes = result.nodes
        nodeIndex = result.nodeIndex
        rootNodeId = result.rootNodeId
        totalValue = result.totalValue
        availableThreads = result.availableThreads
    }
}
