import SwiftUI

struct SettingsView: View {
    @AppStorage(AppSettings.autoStartKey) private var autoStart = true
    @AppStorage(AppSettings.memoryBudgetKey) private var memoryBudget = ""
    @AppStorage(AppSettings.qwen3ASR06BEnabledKey) private var qwen3ASR06BEnabled = false
    @AppStorage(AppSettings.proxyModeKey) private var proxyMode = ProxyMode.system.rawValue
    @AppStorage(AppSettings.httpProxyKey) private var httpProxy = ""
    @AppStorage(AppSettings.httpsProxyKey) private var httpsProxy = ""
    @Environment(DaemonController.self) private var controller

    var body: some View {
        Form {
            Section("守护进程") {
                Toggle("启动应用时自动拉起守护进程", isOn: $autoStart)
                LabeledContent("aiworkd 路径（自动探测）", value: resolvedText)
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
            }

            Section("语音转文字 · STT") {
                Toggle("启用 Qwen3-ASR 0.6B", isOn: $qwen3ASR06BEnabled)
                    .accessibilityLabel("启用 Qwen3-ASR 0.6B 语音转文字")
                    .accessibilityHint("控制 Qwen3-ASR 是否作为可用的语音转文字选项")
                Text("同时装配 MLX 8-bit 与 PyTorch Provider；本机优先使用 MLX 8-bit，PyTorch 保留为兼容回退。修改后需要重启 aiworkd。")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section("网络代理") {
                Picker("代理模式", selection: $proxyMode) {
                    ForEach(ProxyMode.allCases) { mode in
                        Text(mode.title).tag(mode.rawValue)
                    }
                }
                .pickerStyle(.segmented)

                if proxyMode == ProxyMode.manual.rawValue {
                    TextField("HTTP 代理，例如 127.0.0.1:6152", text: $httpProxy)
                        .textFieldStyle(.roundedBorder)
                    TextField("HTTPS 代理，例如 127.0.0.1:6152", text: $httpsProxy)
                        .textFieldStyle(.roundedBorder)
                }

                Text(proxyDescription)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            Section {
                Button {
                    controller.restartDaemon()
                } label: {
                    Label("应用设置并重启 aiworkd", systemImage: "arrow.clockwise.circle")
                }
                .buttonStyle(ProminentButtonStyle())
                .disabled(!settingsAreValid)
            }

            Section("关于") {
                LabeledContent("API 地址", value: "http://127.0.0.1:11435")
            }
        }
        .formStyle(.grouped)
        .frame(maxWidth: 680, maxHeight: .infinity, alignment: .top)
        .frame(maxWidth: .infinity)
        .navigationTitle("设置")
    }

    private var resolvedText: String {
        if let binary = DaemonController.resolveBinary() {
            return binary.path
        }
        return "未找到，请先在 MacAI 仓库构建"
    }

    private var settingsAreValid: Bool {
        let memoryIsValid = memoryBudget.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            || AppSettings.parseMemoryBudget(memoryBudget) != nil
        guard proxyMode == ProxyMode.manual.rawValue else { return memoryIsValid }
        let values = [httpProxy, httpsProxy].filter {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        return memoryIsValid
            && !values.isEmpty
            && values.allSatisfy { AppSettings.normalizedProxyURL($0) != nil }
    }

    private var proxyDescription: String {
        switch ProxyMode(rawValue: proxyMode) ?? .system {
        case .system:
            "自动读取 macOS 网络设置中的 HTTP/HTTPS 代理，模型下载与 Provider 子进程会一并使用。"
        case .disabled:
            "aiworkd 将忽略继承环境和系统设置中的代理，直接连接网络。"
        case .manual:
            settingsAreValid
                ? "留空其中一项即可只配置一种协议；未填写协议不会使用代理。"
                : "请至少填写一个有效地址；可省略 http:// 前缀。"
        }
    }
}
