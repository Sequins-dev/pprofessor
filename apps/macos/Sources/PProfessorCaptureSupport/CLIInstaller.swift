import Foundation

public struct CLIInstaller {
    public struct InstallResult: Equatable, Sendable {
        public let installedURL: URL
        public let installDirectoryIsOnPATH: Bool
    }

    public enum Error: Swift.Error, Equatable {
        case bundledExecutableMissing
    }

    private let bundledExecutableURL: URL
    private let installDirectoryURL: URL
    private let environment: [String: String]
    private let fileManager: FileManager

    public init(
        bundledExecutableURL: URL? = Bundle.main.bundleURL
            .appending(path: "Contents/Helpers/pprofessor"),
        installDirectoryURL: URL = FileManager.default.homeDirectoryForCurrentUser
            .appending(path: ".local/bin", directoryHint: .isDirectory),
        environment: [String: String] = ProcessInfo.processInfo.environment,
        fileManager: FileManager = .default
    ) {
        self.bundledExecutableURL = bundledExecutableURL ?? URL(fileURLWithPath: "")
        self.installDirectoryURL = installDirectoryURL
        self.environment = environment
        self.fileManager = fileManager
    }

    public func install() throws -> InstallResult {
        guard fileManager.isReadableFile(atPath: bundledExecutableURL.path) else {
            throw Error.bundledExecutableMissing
        }

        try fileManager.createDirectory(at: installDirectoryURL, withIntermediateDirectories: true)

        let installedURL = installDirectoryURL.appending(path: "pprofessor")
        if fileManager.fileExists(atPath: installedURL.path) {
            try fileManager.removeItem(at: installedURL)
        }
        try fileManager.copyItem(at: bundledExecutableURL, to: installedURL)
        try fileManager.setAttributes([.posixPermissions: 0o755], ofItemAtPath: installedURL.path)

        return InstallResult(
            installedURL: installedURL,
            installDirectoryIsOnPATH: installDirectoryIsOnPATH()
        )
    }

    private func installDirectoryIsOnPATH() -> Bool {
        let installPath = installDirectoryURL.standardizedFileURL.path
        return environment["PATH", default: ""]
            .split(separator: ":")
            .map(String.init)
            .contains { URL(fileURLWithPath: $0).standardizedFileURL.path == installPath }
    }
}
