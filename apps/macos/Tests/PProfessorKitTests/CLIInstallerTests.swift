import Foundation
import Testing
@testable import PProfessorCaptureSupport

@Suite("CLIInstaller")
struct CLIInstallerTests {
    @Test func installsBundledExecutableToDestinationDirectory() throws {
        let sandbox = try TemporaryDirectory()
        let source = sandbox.url.appending(path: "bundle/pprofessor")
        let destinationDirectory = sandbox.url.appending(path: "home/.local/bin")
        let destination = destinationDirectory.appending(path: "pprofessor")

        try FileManager.default.createDirectory(
            at: source.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try Data("new-binary".utf8).write(to: source)

        let installer = CLIInstaller(
            bundledExecutableURL: source,
            installDirectoryURL: destinationDirectory
        )

        let result = try installer.install()

        #expect(result.installedURL == destination)
        #expect(try String(contentsOf: destination, encoding: .utf8) == "new-binary")
        #expect(isExecutable(destination))
    }

    @Test func overwritesExistingExecutable() throws {
        let sandbox = try TemporaryDirectory()
        let source = sandbox.url.appending(path: "bundle/pprofessor")
        let destinationDirectory = sandbox.url.appending(path: "home/.local/bin")
        let destination = destinationDirectory.appending(path: "pprofessor")

        try FileManager.default.createDirectory(
            at: source.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try FileManager.default.createDirectory(
            at: destinationDirectory,
            withIntermediateDirectories: true
        )
        try Data("replacement".utf8).write(to: source)
        try Data("old".utf8).write(to: destination)

        let installer = CLIInstaller(
            bundledExecutableURL: source,
            installDirectoryURL: destinationDirectory
        )

        _ = try installer.install()

        #expect(try String(contentsOf: destination, encoding: .utf8) == "replacement")
        #expect(isExecutable(destination))
    }

    @Test func reportsMissingBundledExecutable() throws {
        let sandbox = try TemporaryDirectory()
        let installer = CLIInstaller(
            bundledExecutableURL: sandbox.url.appending(path: "missing/pprofessor"),
            installDirectoryURL: sandbox.url.appending(path: "home/.local/bin")
        )

        #expect(throws: CLIInstaller.Error.bundledExecutableMissing) {
            try installer.install()
        }
    }

    private func isExecutable(_ url: URL) -> Bool {
        FileManager.default.isExecutableFile(atPath: url.path)
    }
}

private final class TemporaryDirectory {
    let url: URL

    init() throws {
        url = FileManager.default.temporaryDirectory
            .appending(path: UUID().uuidString, directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
    }

    deinit {
        try? FileManager.default.removeItem(at: url)
    }
}
