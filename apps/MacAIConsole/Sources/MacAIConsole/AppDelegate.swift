import AppKit
import Foundation

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    static weak var shared: AppDelegate?
    weak var controller: DaemonController?

    override init() {
        super.init()
        Self.shared = self
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let controller else { return .terminateNow }
        return confirmTermination(controller: controller)
    }

    /// 退出弹窗的文案与按钮配置。拆分为纯函数以便单元测试验证各种状态分支。
    static func terminationAlertContent(
        phase: DaemonController.Phase,
        info: RuntimeInfo?,
        autoStop: Bool
    ) -> (title: String, informative: String, buttons: [String]) {
        let statusText: String
        switch phase {
        case .online:
            let pidText = info.map { "（PID \($0.pid)）" } ?? ""
            let modelCount = info?.loadedModels.count ?? 0
            let modelText = modelCount > 0 ? "，当前已加载 \(modelCount) 个模型" : ""
            statusText = "后台守护进程（aiworkd）正在运行\(pidText)\(modelText)。"
        case .starting:
            statusText = "后台守护进程（aiworkd）正在启动中。"
        case .stopping:
            statusText = "后台守护进程（aiworkd）正在停止中。"
        case .offline:
            statusText = "后台守护进程（aiworkd）当前未运行。"
        }

        let settingText = autoStop
            ? "当前已开启「退出应用时自动退出守护进程」，退出软件将同时关闭后台守护进程。"
            : "当前未开启「退出应用时自动退出守护进程」，退出软件默认不会关闭后台守护进程。"

        let buttonDescriptions = autoStop
            ? "• 关闭所有：退出 MacAIConsole 并停止后台守护进程，释放系统资源。\n• 取消：返回应用，不执行退出。"
            : "• 关闭所有：退出 MacAIConsole 并停止后台守护进程，释放系统资源。\n• 关闭GUI：仅退出 MacAIConsole 界面，后台守护进程保持运行。\n• 取消：返回应用，不执行退出。"

        let informative = "\(statusText)\n\(settingText)\n\n\(buttonDescriptions)"

        let buttons = autoStop
            ? ["关闭所有", "取消"]
            : ["关闭所有", "关闭GUI", "取消"]

        return (title: "退出 MacAIConsole", informative: informative, buttons: buttons)
    }

    private func confirmTermination(controller: DaemonController) -> NSApplication.TerminateReply {
        let autoStop = AppSettings.autoStopDaemonOnExit
        let content = Self.terminationAlertContent(
            phase: controller.phase,
            info: controller.info,
            autoStop: autoStop
        )

        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = content.title
        alert.informativeText = content.informative

        for title in content.buttons {
            alert.addButton(withTitle: title)
        }

        // 取消按钮绑定 Esc 快捷键
        if let cancelButton = alert.buttons.last, content.buttons.last == "取消" {
            cancelButton.keyEquivalent = "\u{1b}"
        }

        NSApp.activate(ignoringOtherApps: true)
        let response = alert.runModal()

        if autoStop {
            // 选项：["关闭所有", "取消"]
            switch response {
            case .alertFirstButtonReturn:
                controller.stopDaemonForExit(forceStop: true)
                return .terminateNow
            default:
                return .terminateCancel
            }
        } else {
            // 选项：["关闭所有", "关闭GUI", "取消"]
            switch response {
            case .alertFirstButtonReturn:
                controller.stopDaemonForExit(forceStop: true)
                return .terminateNow
            case .alertSecondButtonReturn:
                controller.stopDaemonForExit(forceStop: false)
                return .terminateNow
            default:
                return .terminateCancel
            }
        }
    }
}
