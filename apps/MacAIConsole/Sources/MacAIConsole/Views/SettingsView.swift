import AppKit
import SwiftUI

struct SettingsView: View {
    @AppStorage("aiworkdPath") private var daemonPath = ""
    @AppStorage("autoStartDaemon") private var autoStart = true
    @Environment(DaemonController.self) private var controller

    var body: some View {
        Form {
            Section("守护进程") {
                HStack {
                    TextField("aiworkd 可执行文件路径（留空自动探测）", text: $daemonPath)
                    Button("浏览…") { pickBinary() }
                }
                Toggle("启动应用时自动拉起守护进程", isOn: $autoStart)
                LabeledContent("当前解析结果", value: resolvedText)
                Button("打开日志文件夹") { revealLogs() }
            }

            Section("关于") {
                LabeledContent("API 地址", value: "http://127.0.0.1:11435")
                LabeledContent("版本", value: "0.1.0")
            }
        }
        .formStyle(.grouped)
        .frame(width: 520)
        .task { controller.bootstrapIfNeeded() }
    }

    private var resolvedText: String {
        if let binary = DaemonController.resolveBinary() {
            return binary.path
        }
        return "未找到，请手动指定路径"
    }

    private func pickBinary() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.allowedContentTypes = [.executable]
        if panel.runModal() == .OK, let url = panel.url {
            daemonPath = url.path
        }
    }

    private func revealLogs() {
        let dir = DaemonController.logDirectory
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        NSWorkspace.shared.activateFileViewerSelecting([dir])
    }
}