import SwiftUI

@main
struct MacAIConsoleApp: App {
    private let controller = DaemonController()

    var body: some Scene {
        WindowGroup(id: "main") {
            RootView()
                .environment(controller)
                .frame(minWidth: 780, minHeight: 520)
                .tint(Theme.accent)
                .preferredColorScheme(.dark)
        }
        .windowResizability(.contentMinSize)

        MenuBarExtra {
            MenuBarContentView()
                .environment(controller)
                .tint(Theme.accent)
                .preferredColorScheme(.dark)
        } label: {
            Image(systemName: menuBarIcon)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView()
                .environment(controller)
                .tint(Theme.accent)
                .preferredColorScheme(.dark)
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