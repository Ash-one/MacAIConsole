import SwiftUI

@main
struct MacAIConsoleApp: App {
    @State private var router = AppRouter()
    @State private var updateState = AppUpdateState()
    @AppStorage(AppSettings.appearanceKey) private var appearance = AppearanceMode.system.rawValue
    private let controller = DaemonController()

    var body: some Scene {
        WindowGroup(id: "main") {
            RootView()
                .environment(controller)
                .environment(router)
                .environment(updateState)
                .frame(minWidth: 860, idealWidth: 1000, minHeight: 580, idealHeight: 660)
                .tint(Theme.accent)
                .preferredColorScheme(preferredColorScheme)
                .task {
                    updateState.setInstallHandlers {
                        controller.stopDaemon()
                        // stopDaemon 异步发送 SIGTERM；等待其退出完成，避免新实例与
                        // 垂死的旧 daemon 争抢 11435 端口（与 stopDeadline 的 8 秒对齐）。
                        for _ in 0..<80 where controller.phase == .stopping {
                            try? await Task.sleep(for: .milliseconds(100))
                        }
                    } recovery: {
                        if controller.phase == .offline {
                            controller.startDaemon()
                        }
                    }
                    #if !DEBUG
                    guard AppSettings.autoUpdateCheck else { return }
                    try? await Task.sleep(for: .seconds(3))
                    await updateState.checkSilently()
                    #endif
                }
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
