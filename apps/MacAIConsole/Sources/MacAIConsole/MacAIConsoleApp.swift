import SwiftUI

@main
struct MacAIConsoleApp: App {
    @State private var router = AppRouter()
    private let controller = DaemonController()
    @State private var pythonEnvironments = PythonEnvironmentManager()

    var body: some Scene {
        WindowGroup(id: "main") {
            RootView()
                .environment(controller)
                .environment(router)
                .environment(pythonEnvironments)
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
    }

    private var menuBarIcon: String {
        switch controller.phase {
        case .online: "antenna.radiowaves.left.and.right"
        case .starting, .stopping: "hourglass"
        case .offline: "antenna.radiowaves.left.and.right.slash"
        }
    }
}