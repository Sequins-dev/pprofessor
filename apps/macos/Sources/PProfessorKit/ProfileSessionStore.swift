import SwiftData

@MainActor
public final class ProfileSessionStore {
    public let context: ModelContext

    public init(container: ModelContainer) {
        context = container.mainContext
    }

    public func delete(_ session: ProfileSession) throws {
        context.delete(session)
        try context.save()
    }
}
