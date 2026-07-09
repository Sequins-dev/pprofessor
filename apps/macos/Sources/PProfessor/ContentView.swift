import SwiftUI
import PProfessorKit

struct ContentView: View {
    @State private var viewModel = ProfileViewModel()
    @State private var showFileImporter = false

    var body: some View {
        VStack(spacing: 0) {
            FilterBar(viewModel: viewModel, onOpenFile: { showFileImporter = true })
            mainContent
        }
        .frame(minWidth: 800, minHeight: 500)
        .fileImporter(
            isPresented: $showFileImporter,
            allowedContentTypes: [.item],
            allowsMultipleSelection: false
        ) { result in
            guard case .success(let urls) = result, let url = urls.first else { return }
            Task { await viewModel.loadFile(url: url) }
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
            profileGraph
        }
    }

    @State private var hoveredNodeId: String?
    @State private var selectedNodeId: String?
    @State private var selectedStackTrace: [FlamegraphNode] = []

    @ViewBuilder
    private var profileGraph: some View {
        VStack(spacing: 0) {
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

            if let selectedId = selectedNodeId,
               let selectedNode = viewModel.getNode(nodeId: selectedId) {
                ProfileDetailPanel(
                    node: selectedNode,
                    stackTrace: selectedStackTrace,
                    onClose: { selectedNodeId = nil }
                )
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
}
