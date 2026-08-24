import AppKit
import SwiftUI

/// 菜单栏小窗：状态摘要 + 快捷启停 + 打开主窗口。
struct MenuBarContentView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                StatusBadge(phase: controller.phase)
                Spacer(minLength: 0)
            }
            .padding(.top, 2)

            if let info = controller.info {
                LabeledContent("版本", value: info.version)
                LabeledContent("运行时长", value: Format.uptime(info.uptimeSecs))
                LabeledContent("活跃请求", value: "\(info.activeRequests)")
                LabeledContent("已加载模型", value: "\(info.loadedModels.count)")
            } else if controller.phase == .offline {
                Text("aiworkd 未运行。启动后这里会显示实时状态。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } else {
                Text("正在连接守护进程…")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }

            Divider()

            Button {
                openWindow(id: "main")
                NSApp.activate(ignoringOtherApps: true)
            } label: {
                Label("打开主窗口", systemImage: "macwindow")
            }
            .keyboardShortcut("o")

            switch controller.phase {
            case .online:
                Button(role: .destructive) {
                    controller.stopDaemon()
                } label: {
                    Label("停止 aiworkd", systemImage: "stop.circle")
                }
            case .offline:
                Button {
                    controller.startDaemon()
                } label: {
                    Label("启动 aiworkd", systemImage: "play.circle")
                }
            case .starting, .stopping:
                Button {} label: {
                    Label(controller.phase == .starting ? "正在启动…" : "正在停止…", systemImage: "hourglass")
                }
                .disabled(true)
            }

            Divider()

            SettingsLink {
                Label("设置…", systemImage: "gearshape")
            }
            .keyboardShortcut(",")

            Button("退出 MacAIConsole") {
                NSApp.terminate(nil)
            }
            .keyboardShortcut("q")
        }
        .padding(12)
        .frame(width: 300)
    }
}