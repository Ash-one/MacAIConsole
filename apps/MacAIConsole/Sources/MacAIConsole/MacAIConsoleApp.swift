import SwiftUI

@main
struct MacAIConsoleApp: App {
    @State private var router = AppRouter()
    @AppStorage(AppSettings.appearanceKey) private var appearance = AppearanceMode.system.rawValue
    private let controller = DaemonController()

    var body: some Scene {
        WindowGroup(id: "main") {
            RootView()
                .environment(controller)
                .environment(router)
                .frame(minWidth: 860, idealWidth: 1000, minHeight: 580, idealHeight: 660)
                .tint(Theme.accent)
                .preferredColorScheme(preferredColorScheme)
        }
        .defaultSize(width: 1000, height: 660)
        .windowResizability(.contentMinSize)

        MenuBarExtra {
            MenuBarContentView()
                .environment(controller)
                .tint(Theme.accent)
                .preferredColorScheme(preferredColorScheme)
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

    private var preferredColorScheme: ColorScheme? {
        switch AppearanceMode(rawValue: appearance) ?? .system {
        case .system: nil
        case .light: .light
        case .dark: .dark
        }
    }
}
