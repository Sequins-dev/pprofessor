import Foundation
import PProfessorKit
import SwiftUI

struct ProcessPickerView: View {
    let onAttach: (AttachProcessInfo) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var processes: [AttachProcessInfo] = []
    @State private var selection: AttachProcessInfo.ID?
    @State private var search = ""
    @State private var error: String?

    private var filtered: [AttachProcessInfo] {
        guard !search.isEmpty else { return processes }
        return processes.filter {
            $0.name.localizedCaseInsensitiveContains(search)
                || ($0.executablePath?.localizedCaseInsensitiveContains(search) ?? false)
                || String($0.pid).contains(search)
        }
    }

    private var selectedProcess: AttachProcessInfo? {
        guard let selection else { return nil }
        return processes.first(where: { $0.id == selection })
    }

    var body: some View {
        VStack(spacing: 12) {
            Text("Attach to Process").font(.headline)
            TextField("Search name, path, or PID", text: $search)
                .textFieldStyle(.roundedBorder)
            List(filtered, selection: $selection) { process in
                HStack {
                    VStack(alignment: .leading) {
                        Text(process.name)
                        Text(process.executablePath ?? "PID \(process.pid)")
                            .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer()
                    Text(process.architecture).font(.caption).foregroundStyle(.secondary)
                    Text(String(process.pid)).font(.caption.monospacedDigit())
                }
                .tag(process.id)
            }
            if let error { Text(error).font(.caption).foregroundStyle(.red) }
            HStack {
                Button("Cancel") { dismiss() }
                Spacer()
                Button("Attach") {
                    guard let process = selectedProcess, process.canAttach else { return }
                    onAttach(process)
                    dismiss()
                }
                .buttonStyle(.borderedProminent)
                .disabled(selectedProcess?.canAttach != true)
            }
        }
        .padding()
        .frame(width: 620, height: 480)
        .task {
            while !Task.isCancelled {
                await refresh()
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    private func refresh() async {
        do {
            processes = AttachProcessInfo.pickerTargets(
                try await ProcessListLoader.load(),
                excludingPID: UInt32(ProcessInfo.processInfo.processIdentifier)
            )
            error = nil
        } catch {
            self.error = error.localizedDescription
        }
    }
}

enum ProcessListLoader {
    static func load() async throws -> [AttachProcessInfo] {
        let helper = Bundle.main.bundleURL.appending(path: "Contents/Helpers/pprofessor")
        guard FileManager.default.isExecutableFile(atPath: helper.path) else {
            throw CocoaError(.fileNoSuchFile)
        }
        return try await Task.detached {
            let process = Process()
            let pipe = Pipe()
            process.executableURL = helper
            process.arguments = ["processes", "--json"]
            process.standardOutput = pipe
            try process.run()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            guard process.terminationStatus == 0 else { throw CocoaError(.executableNotLoadable) }
            let decoder = JSONDecoder()
            decoder.keyDecodingStrategy = .convertFromSnakeCase
            return try decoder.decode([AttachProcessInfo].self, from: data)
        }.value
    }
}
