import AppKit
import SwiftUI
import PProfessorKit
import SwiftData

@main
struct PProfessorApp: App {
    private let modelContainer: ModelContainer
    private let coordinator: SessionCoordinator

    init() {
        do {
            let container = try ModelContainer(for: ProfileSession.self)
            modelContainer = container
            coordinator = SessionCoordinator(container: container)
        } catch {
            fatalError("Unable to initialize profile session storage: \(error)")
        }
        if let iconURL = Bundle.main.url(forResource: "AppIcon", withExtension: "icns"),
           let icon = NSImage(contentsOf: iconURL) {
            NSApplication.shared.applicationIconImage = icon
        }
    }

    var body: some Scene {
        WindowGroup {
            ContentView(coordinator: coordinator)
        }
        .windowToolbarStyle(.unifiedCompact(showsTitle: false))
        .modelContainer(modelContainer)
        .commands {
            CommandGroup(replacing: .newItem) {}
            CommandMenu("Tools") {
                Button("Install CLI tools") {
                    installCLITools()
                }
            }
        }
    }

    private func installCLITools() {
        do {
            let result = try CLIInstaller().install()
            let message: String
            if result.installDirectoryIsOnPATH {
                message = "Installed pprofessor to \(result.installedURL.path)."
            } else {
                message = """
                Installed pprofessor to \(result.installedURL.path).

                Add this directory to your PATH:
                export PATH="$HOME/.local/bin:$PATH"
                """
            }
            showAlert(title: "CLI tools installed", message: message)
        } catch CLIInstaller.Error.bundledExecutableMissing {
            showAlert(
                title: "CLI tools unavailable",
                message: "The bundled pprofessor executable was not found in this app build."
            )
        } catch {
            showAlert(title: "CLI install failed", message: error.localizedDescription)
        }
    }

    private func showAlert(title: String, message: String) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.alertStyle = .informational
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }
}
