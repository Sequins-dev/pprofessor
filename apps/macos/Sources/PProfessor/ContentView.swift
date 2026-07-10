import SwiftUI
import PProfessorKit
import SwiftData

struct ContentView: View {
    let coordinator: SessionCoordinator
    @State private var viewModel = ProfileViewModel()
    @State private var showFileImporter = false
    @State private var showProcessPicker = false
    @State private var selectedSessionID: UUID?
    @Query(sort: \ProfileSession.startedAt, order: .reverse) private var sessions: [ProfileSession]

    var body: some View {
        NavigationSplitView {
            List(selection: $selectedSessionID) {
                let live = sessions.filter { $0.status == .live || $0.status == .finalizing || $0.status == .connecting }
                if !live.isEmpty {
                    Section("Live") { ForEach(live) { sessionRow($0) } }
                }
                Section("Sessions") {
                    ForEach(sessions.filter { !live.contains($0) }) { sessionRow($0) }
                }
            }
            .navigationTitle("Profiles")
            .frame(minWidth: 220)
        } detail: {
            mainContent
        }
        .toolbar {
            ProfileToolbar(
                viewModel: viewModel,
                onOpenFile: { showFileImporter = true },
                onAttach: { showProcessPicker = true }
            )
        }
        .frame(minWidth: 800, minHeight: 500)
        .task { coordinator.start() }
        .onChange(of: selectedSessionID) { _, id in
            guard let id, let session = sessions.first(where: { $0.id == id }) else { return }
            coordinator.select(session, viewModel: viewModel)
        }
        .sheet(isPresented: $showProcessPicker) {
            ProcessPickerView { process in
                do { try coordinator.launchAttach(process: process) }
                catch { coordinator.lastError = error.localizedDescription }
            }
        }
        .fileImporter(
            isPresented: $showFileImporter,
            allowedContentTypes: [.item],
            allowsMultipleSelection: false
        ) { result in
            guard case .success(let urls) = result, let url = urls.first else { return }
            Task { await coordinator.importProfile(url: url, viewModel: viewModel) }
        }
    }

    private func sessionRow(_ session: ProfileSession) -> some View {
        HStack {
            Image(systemName: session.status == .live ? "record.circle.fill" : "flame")
                .foregroundStyle(session.status == .live ? .red : .secondary)
            VStack(alignment: .leading) {
                Text(session.displayName).lineLimit(1)
                Text(session.status.rawValue.capitalized)
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .tag(session.id)
        .contextMenu {
            if session.status == .live { Button("Stop") { coordinator.stopCapture(session) } }
            Button("Delete", role: .destructive) { coordinator.delete(session) }
        }
    }

    @ViewBuilder
    private var mainContent: some View {
        if let error = viewModel.error {
            VStack {
                Spacer()
                Image(systemName: "exclamationmark.triangle")
                    .font(.system(size: 40))
                    .foregroundStyle(.secondary)
                Text(error)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .padding(.horizontal)
                Spacer()
            }
        } else if viewModel.isLoading {
            ProfileEmptyState(isLoading: true, onOpenFile: nil)
        } else if viewModel.nodes.isEmpty {
            ProfileEmptyState(isLoading: false, onOpenFile: { showFileImporter = true })
        } else {
            profileContent
        }
    }

    private var profileContent: some View {
        VStack(spacing: 0) {
            if let timeline = viewModel.timeline,
               let selection = viewModel.timelineSelection {
                ProfileTimelineView(
                    timeline: timeline,
                    range: selection.range,
                    onRangeChange: { range, isDragging in
                        viewModel.selectTimeRange(range, isDragging: isDragging)
                    },
                    onReset: { viewModel.resetTimeRange() }
                )
                Divider()
            }
            profileGraph
        }
    }

    @State private var hoveredNodeId: String?
    @State private var selectedNodeId: String?
    @State private var selectedStackTrace: [FlamegraphNode] = []

    @ViewBuilder
    private var profileGraph: some View {
        Group {
            if let selectedId = selectedNodeId,
               let selectedNode = viewModel.getNode(nodeId: selectedId) {
                VSplitView {
                    flamegraphCanvas
                        .frame(minHeight: 120)

                ProfileDetailPanel(
                    node: selectedNode,
                    stackTrace: selectedStackTrace,
                    onClose: { selectedNodeId = nil }
                )
                    .frame(minHeight: 140, idealHeight: 300)
                }
            } else {
                flamegraphCanvas
            }
        }
        .onChange(of: selectedNodeId) { _, newValue in
            if let id = newValue {
                selectedStackTrace = viewModel.getStackTrace(for: id)
            } else {
                selectedStackTrace = []
            }
        }
    }

    private var flamegraphCanvas: some View {
        GeometryReader { geo in
            ZStack(alignment: .topTrailing) {
                CanvasIcicleGraphView(
                    nodes: viewModel.nodes,
                    nodeIndex: viewModel.nodeIndex,
                    rootNodeId: viewModel.rootNodeId,
                    width: geo.size.width,
                    availableHeight: geo.size.height,
                    hoveredNodeId: $hoveredNodeId,
                    selectedNodeId: $selectedNodeId,
                    searchText: viewModel.searchText,
                    zoomedNodeId: $viewModel.zoomedNodeId
                )
                if let zoomedId = viewModel.zoomedNodeId,
                   let zoomedNode = viewModel.getNode(nodeId: zoomedId) {
                    ZoomIndicatorView(frameName: zoomedNode.functionName) {
                        viewModel.zoomedNodeId = nil
                    }
                    .padding(12)
                }
            }
        }
        .background(Color(NSColor.textBackgroundColor))
    }
}
