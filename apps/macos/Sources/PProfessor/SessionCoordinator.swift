import Foundation
import Observation
import PProfessorKit
import SwiftData

private struct SessionHelloPayload: Decodable {
    let sessionId: String
    let mode: String
    let pid: UInt32
    let processName: String
    let frequencyHz: UInt32
}

private final class RuntimeSession {
    var accumulator = LiveProfileAccumulator()
    var lastSequence: UInt64 = 0
}

@MainActor
@Observable
final class SessionCoordinator {
    private let sessionStore: ProfileSessionStore
    private var modelContext: ModelContext { sessionStore.context }
    private let storageRoot: URL
    private var server: UnixSessionServer?
    private var connectionSessions: [UUID: UUID] = [:]
    private var runtimes: [UUID: RuntimeSession] = [:]
    private var launchedProcesses: [UUID: Process] = [:]
    private var sessionOutputs: [UUID: URL] = [:]

    var selectedSessionID: UUID?
    var onSelectedProfile: ((DecodedProfile) -> Void)?
    var onSelectedArtifact: ((URL) -> Void)?
    var lastError: String?

    init(container: ModelContainer) {
        sessionStore = ProfileSessionStore(container: container)
        storageRoot = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("PProfessor/Sessions", isDirectory: true)
    }

    func start() {
        guard server == nil else { return }
        do {
            try FileManager.default.createDirectory(at: storageRoot, withIntermediateDirectories: true)
            let socketPath = FileManager.default.temporaryDirectory.appendingPathComponent("pprofessor-v1.sock").path
            let server = UnixSessionServer(path: socketPath) { [weak self] connectionID, frame in
                Task { @MainActor in self?.receive(connectionID: connectionID, frame: frame) }
            }
            try server.start()
            self.server = server
            markAbandonedSessionsInterrupted()
        } catch {
            lastError = error.localizedDescription
        }
    }

    func stop() {
        server?.stop()
        server = nil
    }

    func select(_ session: ProfileSession, viewModel: ProfileViewModel) {
        selectedSessionID = session.id
        onSelectedProfile = { [weak viewModel] profile in
            viewModel?.loadDecodedProfile(profile, resetTimelineSelection: false)
        }
        onSelectedArtifact = { [weak viewModel] url in
            guard let viewModel else { return }
            Task { await viewModel.loadFile(url: url) }
        }
        if let runtime = runtimes[session.id] {
            viewModel.loadDecodedProfile(runtime.accumulator.profile, resetTimelineSelection: true)
        } else if let relative = session.artifactRelativePath {
            Task { await viewModel.loadFile(url: storageRoot.appendingPathComponent(relative)) }
        } else if let relative = session.journalRelativePath,
                  let profile = replayJournal(at: storageRoot.appendingPathComponent(relative), sessionID: session.id) {
            viewModel.loadDecodedProfile(profile, resetTimelineSelection: true)
        }
    }

    func importProfile(url: URL, viewModel: ProfileViewModel) async {
        let id = UUID()
        do {
            let directory = try sessionDirectory(id: id)
            let destination = directory.appendingPathComponent("profile.pb.gz")
            try FileManager.default.copyItem(at: url, to: destination)
            let session = ProfileSession(
                id: id,
                displayName: url.deletingPathExtension().deletingPathExtension().lastPathComponent,
                source: .imported,
                status: .completed,
                frequencyHz: 0
            )
            session.artifactRelativePath = relativePath(destination)
            session.endedAt = Date()
            modelContext.insert(session)
            try modelContext.save()
            selectedSessionID = id
            await viewModel.loadFile(url: destination)
        } catch {
            lastError = error.localizedDescription
        }
    }

    func delete(_ session: ProfileSession) {
        if session.status == .live { stopCapture(session) }
        do {
            try sessionStore.delete(session)
            try? FileManager.default.removeItem(
                at: storageRoot.appendingPathComponent(session.id.uuidString, isDirectory: true)
            )
            runtimes.removeValue(forKey: session.id)
            launchedProcesses.removeValue(forKey: session.id)
            connectionSessions = connectionSessions.filter { $0.value != session.id }
            if selectedSessionID == session.id { selectedSessionID = nil }
        } catch {
            lastError = error.localizedDescription
        }
    }

    func stopCapture(_ session: ProfileSession) {
        launchedProcesses[session.id]?.interrupt()
    }

    func launchAttach(process: AttachProcessInfo, frequency: Int = 99) throws {
        let helper = Bundle.main.bundleURL.appending(path: "Contents/Helpers/pprofessor")
        guard FileManager.default.isExecutableFile(atPath: helper.path) else {
            throw CocoaError(.fileNoSuchFile)
        }
        let id = UUID()
        let output = FileManager.default.temporaryDirectory
            .appendingPathComponent("pprofessor-app-\(UUID().uuidString).pb.gz")
        let processTask = Process()
        let errorPipe = Pipe()
        processTask.executableURL = helper
        processTask.arguments = [
            "attach", "--freq", String(frequency), "--output", output.path,
            "--expected-start-time", String(process.startTimeMicros),
            "--session-id", id.uuidString, String(process.pid),
        ]
        processTask.standardError = errorPipe
        let session = ProfileSession(
            id: id,
            displayName: process.name,
            source: .appAttach,
            status: .connecting,
            pid: Int(process.pid),
            processName: process.name,
            architecture: process.architecture,
            frequencyHz: frequency
        )
        modelContext.insert(session)
        try modelContext.save()
        processTask.terminationHandler = { [weak self] processTask in
            let message = String(data: errorPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8)
            Task { @MainActor in
                self?.captureTerminated(id: id, status: processTask.terminationStatus, message: message)
            }
        }
        do {
            try processTask.run()
        } catch {
            session.status = .failed
            session.errorSummary = error.localizedDescription
            session.endedAt = Date()
            session.updatedAt = Date()
            try? modelContext.save()
            try? FileManager.default.removeItem(at: output)
            throw error
        }
        launchedProcesses[id] = processTask
        sessionOutputs[id] = output
    }

    private func receive(connectionID: UUID, frame: SessionFrame) {
        if frame.header.kind == .hello {
            receiveHello(connectionID: connectionID, payload: frame.payload)
            return
        }
        guard let sessionID = connectionSessions[connectionID],
              let runtime = runtimes[sessionID],
              let session = fetchSession(id: sessionID)
        else { return }
        guard frame.header.sequence > runtime.lastSequence else { return }
        runtime.lastSequence = frame.header.sequence

        switch frame.header.kind {
        case .profileCheckpoint:
            runtime.accumulator.replace(with: DecodedProfile.decode(from: frame.payload))
            update(session: session, from: runtime.accumulator.profile, status: .live)
        case .profileDelta:
            runtime.accumulator.merge(delta: DecodedProfile.decode(from: frame.payload))
            update(session: session, from: runtime.accumulator.profile, status: .live)
            appendJournal(sessionID: sessionID, frame: frame)
        case .finalizing:
            session.status = .finalizing
            session.updatedAt = Date()
            try? modelContext.save()
        case .finalProfile:
            complete(session: session, gzipProfile: frame.payload)
        case .failed:
            session.status = .failed
            session.errorSummary = String(data: frame.payload, encoding: .utf8)
            session.endedAt = Date()
            try? modelContext.save()
        default:
            break
        }
    }

    private func receiveHello(connectionID: UUID, payload: Data) {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let hello = try? decoder.decode(SessionHelloPayload.self, from: payload),
              let id = UUID(uuidString: hello.sessionId)
        else { return }
        connectionSessions[connectionID] = id
        let existingSession = fetchSession(id: id)
        if runtimes[id] == nil,
           let relative = existingSession?.journalRelativePath {
            _ = replayJournal(
                at: storageRoot.appendingPathComponent(relative),
                sessionID: id
            )
        }
        runtimes[id] = runtimes[id] ?? RuntimeSession()
        if existingSession == nil {
            let session = ProfileSession(
                id: id,
                displayName: hello.processName,
                source: hello.mode == "run" ? .cliRun : .cliAttach,
                status: .live,
                pid: Int(hello.pid),
                processName: hello.processName,
                frequencyHz: Int(hello.frequencyHz)
            )
            session.journalRelativePath = "\(id.uuidString)/live.journal"
            modelContext.insert(session)
            try? modelContext.save()
        } else if let session = existingSession {
            session.status = .live
            session.updatedAt = Date()
            try? modelContext.save()
        }
    }

    private func update(session: ProfileSession, from profile: DecodedProfile, status: ProfileSessionStatus) {
        session.status = status
        session.durationNanos = profile.durationNanos
        session.sampleCount = profile.samples.reduce(0) { partial, sample in
            partial + (sample.values.first ?? 0)
        }
        session.updatedAt = Date()
        try? modelContext.save()
        if selectedSessionID == session.id { onSelectedProfile?(profile) }
    }

    private func complete(session: ProfileSession, gzipProfile: Data) {
        do {
            let directory = try sessionDirectory(id: session.id)
            let temporary = directory.appendingPathComponent("profile.pb.gz.tmp")
            let destination = directory.appendingPathComponent("profile.pb.gz")
            try gzipProfile.write(to: temporary, options: .atomic)
            if FileManager.default.fileExists(atPath: destination.path) { try FileManager.default.removeItem(at: destination) }
            try FileManager.default.moveItem(at: temporary, to: destination)
            try? FileManager.default.removeItem(at: directory.appendingPathComponent("live.journal"))
            session.artifactRelativePath = relativePath(destination)
            session.journalRelativePath = nil
            session.status = .completed
            session.endedAt = Date()
            session.updatedAt = Date()
            try modelContext.save()
            if let output = sessionOutputs.removeValue(forKey: session.id) {
                try? FileManager.default.removeItem(at: output)
            }
            if selectedSessionID == session.id {
                onSelectedArtifact?(destination)
            }
        } catch {
            session.status = .failed
            session.errorSummary = error.localizedDescription
            try? modelContext.save()
        }
    }

    private func appendJournal(sessionID: UUID, frame: SessionFrame) {
        guard let directory = try? sessionDirectory(id: sessionID) else { return }
        let url = directory.appendingPathComponent("live.journal")
        let data = frame.header.encoded() + frame.payload
        if !FileManager.default.fileExists(atPath: url.path) { FileManager.default.createFile(atPath: url.path, contents: nil) }
        guard let handle = try? FileHandle(forWritingTo: url) else { return }
        defer { try? handle.close() }
        do {
            try handle.seekToEnd()
            try handle.write(contentsOf: data)
        } catch {}
    }

    private func replayJournal(at url: URL, sessionID: UUID) -> DecodedProfile? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        var parser = SessionFrameParser()
        guard let frames = try? parser.append(data) else { return nil }
        let runtime = RuntimeSession()
        for frame in frames {
            switch frame.header.kind {
            case .profileCheckpoint:
                runtime.accumulator.replace(with: DecodedProfile.decode(from: frame.payload))
            case .profileDelta:
                runtime.accumulator.merge(delta: DecodedProfile.decode(from: frame.payload))
            default:
                continue
            }
            runtime.lastSequence = max(runtime.lastSequence, frame.header.sequence)
        }
        runtimes[sessionID] = runtime
        return runtime.accumulator.profile
    }

    private func sessionDirectory(id: UUID) throws -> URL {
        let directory = storageRoot.appendingPathComponent(id.uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory
    }

    private func relativePath(_ url: URL) -> String {
        url.path.replacingOccurrences(of: storageRoot.path + "/", with: "")
    }

    private func fetchSession(id: UUID) -> ProfileSession? {
        let descriptor = FetchDescriptor<ProfileSession>(predicate: #Predicate { $0.id == id })
        return try? modelContext.fetch(descriptor).first
    }

    private func markAbandonedSessionsInterrupted() {
        let descriptor = FetchDescriptor<ProfileSession>(predicate: #Predicate {
            $0.statusRaw == "live" || $0.statusRaw == "finalizing" || $0.statusRaw == "connecting"
        })
        if let sessions = try? modelContext.fetch(descriptor) {
            for session in sessions { session.status = .interrupted }
            try? modelContext.save()
        }
    }

    private func captureTerminated(id: UUID, status: Int32, message: String?) {
        launchedProcesses.removeValue(forKey: id)
        guard let session = fetchSession(id: id),
              session.status == .connecting || session.status == .live || session.status == .finalizing
        else { return }
        if status != 0 {
            session.status = .failed
            session.errorSummary = message?.trimmingCharacters(in: .whitespacesAndNewlines)
            session.endedAt = Date()
            session.updatedAt = Date()
            try? modelContext.save()
        }
    }
}

struct AttachProcessInfo: Codable, Identifiable, Hashable {
    let pid: UInt32
    let parentPid: UInt32
    let uid: UInt32
    let name: String
    let executablePath: String?
    let startTimeMicros: UInt64
    let architecture: String

    var id: String { "\(pid)-\(startTimeMicros)" }
}
