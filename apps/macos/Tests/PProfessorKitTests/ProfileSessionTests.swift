import Foundation
import SwiftData
import Testing
@testable import PProfessorKit

@Suite("Profile session persistence")
struct ProfileSessionTests {
    @Test func storesSessionMetadataInSwiftData() throws {
        let container = try ModelContainer(
            for: ProfileSession.self,
            configurations: ModelConfiguration(isStoredInMemoryOnly: true)
        )
        let context = ModelContext(container)
        let id = UUID()
        context.insert(ProfileSession(
            id: id,
            displayName: "node",
            source: .cliAttach,
            status: .live,
            pid: 42,
            frequencyHz: 99,
            startedAt: Date(timeIntervalSince1970: 100)
        ))
        try context.save()

        let sessions = try context.fetch(FetchDescriptor<ProfileSession>())
        #expect(sessions.count == 1)
        #expect(sessions[0].id == id)
        #expect(sessions[0].source == .cliAttach)
        #expect(sessions[0].status == .live)
        #expect(sessions[0].pid == 42)
    }

    @Test @MainActor func sessionStoreDeletesModelsFromTheObservedMainContext() throws {
        let container = try ModelContainer(
            for: ProfileSession.self,
            configurations: ModelConfiguration(isStoredInMemoryOnly: true)
        )
        let session = ProfileSession(
            displayName: "node",
            source: .cliRun,
            status: .completed,
            frequencyHz: 99
        )
        container.mainContext.insert(session)
        try container.mainContext.save()
        let store = ProfileSessionStore(container: container)

        try store.delete(session)

        #expect(try container.mainContext.fetch(FetchDescriptor<ProfileSession>()).isEmpty)
    }
}
