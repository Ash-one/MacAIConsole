import AppKit
import SwiftUI

struct SettingsView: View {
    @AppStorage(AppSettings.aiworkdPathKey) private var daemonPath = ""
    @AppStorage(AppSettings.autoStartKey) private var autoStart = true
    @AppStorage(AppSettings.memoryBudgetKey) private var memoryBudget = ""
    @AppStorage(AppSettings.qwen3ASR06BEnabledKey) private var qwen3ASR06BEnabled = false
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

            Section("资源调度") {
                HStack {
                    TextField("自动（按物理内存计算）", text: $memoryBudget)
                        .textFieldStyle(.roundedBorder)
                    Text("例如 8G、8192M")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                LabeledContent("当前生效预算", value: Format.bytes(controller.info?.memoryBudget))
                Text("留空使用自动策略：物理内存的 75%，且最多保留 8 GB 给系统。修改后需要重启 aiworkd 才会生效。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button {
                    controller.restartDaemon()
                } label: {
                    Label("重启并应用内存预算", systemImage: "arrow.clockwise.circle")
                }
                .buttonStyle(.borderedProminent)
                .disabled(AppSettings.parseMemoryBudget(memoryBudget) == nil && !memoryBudget.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }

            Section("语音转文字 · STT") {
                Toggle("启用 Qwen3-ASR 0.6B", isOn: $qwen3ASR06BEnabled)
                    .accessibilityLabel("启用 Qwen3-ASR 0.6B 语音转文字")
                    .accessibilityHint("控制 Qwen3-ASR 是否作为可用的语音转文字选项")
                Text("同时装配 MLX 8-bit 与 PyTorch Provider；本机优先使用 MLX 8-bit，PyTorch 保留为兼容回退。修改后需要重启 aiworkd。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Button {
                    controller.restartDaemon()
                } label: {
                    Label("重启并应用 STT 设置", systemImage: "arrow.clockwise.circle")
                }
                .buttonStyle(.borderedProminent)
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