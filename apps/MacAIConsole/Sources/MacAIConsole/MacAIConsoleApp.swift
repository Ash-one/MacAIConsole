import SwiftUI

@main
struct MacAIConsoleApp: App {
    private let controller = DaemonController()

    var body: some Scene {
        WindowGroup(id: "main") {
            RootView()
                .environment(controller)
                .frame(minWidth: 780, minHeight: 520)
        }
        .windowResizability(.contentMinSize)

        MenuBarExtra {
            MenuBarContentView()
                .environment(controller)
        } label: {
            Image(systemName: menuBarIcon)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView()
                .environment(controller)
        }
    }

    private var menuBarIcon: String {
        switch controller.phase {
        case .online: "antenna.radiowaves.left.and.right"
        case .starting, .stopping: "hourglass"
        case .offline: "antenna.radiowaves.left.and.right.slash"
        }
    }
}