import SwiftUI
import AppKit

struct FilterBar: View {
    @Bindable var viewModel: ProfileViewModel
    let onOpenFile: () -> Void

    var body: some View {
        HStack(spacing: 12) {
            openFileButton

            Divider()
                .frame(height: 20)

            searchField

            if !viewModel.availableValueTypes.isEmpty {
                valueTypeMenu(types: viewModel.availableValueTypes)

                Divider()
                    .frame(height: 20)
            }

            if !viewModel.availableThreads.isEmpty {
                threadMenu(threads: viewModel.availableThreads)

                Divider()
                    .frame(height: 20)
            }

            ExportButton {
                Button("Export as JSON") {
                    viewModel.exportAsJSON()
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Color(NSColor.windowBackgroundColor))
        .overlay(alignment: .bottom) {
            Divider()
        }
    }

    private var openFileButton: some View {
        Button(action: onOpenFile) {
            HStack(spacing: 4) {
                Image(systemName: "folder")
                    .font(.caption)
                Text("Open File...")
                    .font(.caption)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.quaternary)
            .cornerRadius(4)
        }
        .buttonStyle(.plain)
    }

    private var searchField: some View {
        HStack(spacing: 4) {
            Image(systemName: "magnifyingglass")
                .font(.caption)
                .foregroundStyle(.secondary)
            TextField("Search frames...", text: $viewModel.searchText)
                .textFieldStyle(.plain)
                .font(.caption)
                .frame(width: 120)
            if !viewModel.searchText.isEmpty {
                Button(action: { viewModel.searchText = "" }) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.secondary)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
        .background(.quaternary)
        .cornerRadius(4)
        .fixedSize()
    }

    private func threadMenu(threads: [String]) -> some View {
        let label = viewModel.selectedThread ?? "All Threads"
        return Menu {
            Button(action: { viewModel.selectedThread = nil }) {
                HStack {
                    Text("All Threads")
                    if viewModel.selectedThread == nil {
                        Image(systemName: "checkmark")
                    }
                }
            }
            Divider()
            ForEach(threads, id: \.self) { thread in
                Button(action: { viewModel.selectedThread = thread }) {
                    HStack {
                        Text(thread)
                        if viewModel.selectedThread == thread {
                            Image(systemName: "checkmark")
                        }
                    }
                }
            }
        } label: {
            HStack(spacing: 4) {
                Image(systemName: "cpu")
                    .font(.caption)
                Text(label)
                    .font(.caption)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.quaternary)
            .cornerRadius(4)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
    }

    private func valueTypeMenu(types: [(type: String, unit: String)]) -> some View {
        let selectedLabel = types.indices.contains(viewModel.selectedValueTypeIndex)
            ? types[viewModel.selectedValueTypeIndex].type
            : types.first?.type ?? "Type"

        return Menu {
            ForEach(types.indices, id: \.self) { i in
                Button(action: { viewModel.selectedValueTypeIndex = i }) {
                    HStack {
                        Text("\(types[i].type) (\(types[i].unit))")
                        if viewModel.selectedValueTypeIndex == i {
                            Image(systemName: "checkmark")
                        }
                    }
                }
            }
        } label: {
            HStack(spacing: 4) {
                Text(selectedLabel)
                    .font(.caption)
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.quaternary)
            .cornerRadius(4)
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
    }
}
