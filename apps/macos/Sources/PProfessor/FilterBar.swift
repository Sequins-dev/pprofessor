import SwiftUI
import PProfessorKit

struct ProfileToolbar: ToolbarContent {
    @Bindable var viewModel: ProfileViewModel
    let onOpenFile: () -> Void
#if !APP_STORE
    let onAttach: () -> Void
#endif

    var body: some ToolbarContent {
        ToolbarItemGroup(placement: .navigation) {
            Button(action: onOpenFile) {
                Label("Open Profile", systemImage: "folder")
            }
            .help("Open a pprof profile")

#if !APP_STORE
            Button(action: onAttach) {
                Label("Attach", systemImage: "scope")
            }
            .help("Attach to a live process")
#endif

            Picker("Sample Type", selection: $viewModel.selectedValueTypeIndex) {
                if viewModel.availableValueTypes.isEmpty {
                    Text("Sample Type").tag(0)
                } else {
                    ForEach(viewModel.availableValueTypes.indices, id: \.self) { index in
                        let valueType = viewModel.availableValueTypes[index]
                        Text("\(valueType.type) (\(valueType.unit))").tag(index)
                    }
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .frame(width: 170)
            .disabled(viewModel.availableValueTypes.isEmpty)
            .help((["Select the profile sample type"] + viewModel.profileComments).joined(separator: "\n"))

            if viewModel.availableHeaps.count > 1 {
                Picker("Heap", selection: $viewModel.selectedHeap) {
                    ForEach(viewModel.availableHeaps, id: \.self) { heap in Text(heap).tag(Optional(heap)) }
                }
                .help("Select one heap domain; backing memory and suballocations may overlap")
            }
            if viewModel.hasNegativeValues {
                Picker("Difference", selection: $viewModel.valueSign) {
                    ForEach(ProfileValueSign.allCases, id: \.self) { sign in Text(sign.rawValue).tag(sign) }
                }
                .help("View increases or decreases in the signed pprof difference")
            }

            Picker("Thread", selection: $viewModel.selectedThread) {
                Text("All Threads").tag(nil as String?)
                ForEach(viewModel.availableThreads, id: \.self) { thread in
                    Text(thread).tag(Optional(thread))
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .frame(width: 180)
            .disabled(viewModel.availableThreads.isEmpty)
            .help("Filter the profile by thread")
        }

        ToolbarItem(placement: .principal) {
            HStack(spacing: 0) {
                TextField("Search frames", text: $viewModel.searchText)
                    .textFieldStyle(.roundedBorder)
                    .overlay(alignment: .trailing) {
                        if !viewModel.searchText.isEmpty {
                            Button {
                                viewModel.searchText = ""
                            } label: {
                                Image(systemName: "xmark.circle.fill")
                                    .foregroundStyle(.secondary)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Clear search")
                            .padding(.trailing, 5)
                        }
                    }
            }
            .frame(minWidth: 160, idealWidth: 360, maxWidth: .infinity)
            .help("Search frames")
        }

        ToolbarItem(placement: .primaryAction) {
            Menu {
                if viewModel.hasProfile {
                    Button("Save Profile...") { viewModel.saveProfile() }
                }
                Button("Export as JSON") { viewModel.exportAsJSON() }
            } label: {
                Label("Export", systemImage: "square.and.arrow.up")
            }
        }
    }
}
