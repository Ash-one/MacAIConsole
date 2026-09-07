import SwiftUI

struct SettingsView: View {
    @AppStorage(AppSettings.autoStartKey) private var autoStart = true
    @AppStorage(AppSettings.memoryBudgetKey) private var memoryBudget = ""
    @AppStorage(AppSettings.appearanceKey) private var appearance = AppearanceMode.system.rawValue
    @AppStorage(AppSettings.proxyModeKey) private var proxyMode = ProxyMode.system.rawValue
    @AppStorage(AppSettings.httpProxyKey) private var httpProxy = ""
    @AppStorage(AppSettings.httpsProxyKey) private var httpsProxy = ""
    @AppStorage(AppSettings.downloadSourceKey) private var downloadSource = ModelDownloadSource.official.rawValue
    @AppStorage(AppSettings.customDownloadEndpointKey) private var customDownloadEndpoint = ""
    @Environment(DaemonController.self) private var controller

    var body: some View {
        Form {
            Section("外观") {
                HStack(spacing: 8) {
                    HStack(spacing: 6) {
                        Text("应用外观")
                        InfoTip(text: appearanceDescription)
                    }
                    Spacer(minLength: 12)
                    Picker("应用外观", selection: $appearance) {
                        ForEach(AppearanceMode.allCases) { mode in
                            Text(mode.title).tag(mode.rawValue)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.segmented)
                }
            }

            Section("守护进程") {
                Toggle("启动应用时自动拉起守护进程", isOn: $autoStart)
                LabeledContent("aiworkd 路径（自动探测）", value: resolvedText)
            }

            Section("资源调度") {
                HStack {
                    TextField("自动（按物理内存计算）", text: $memoryBudget)
                        .textFieldStyle(.roundedBorder)
                    InfoTip(text: memoryBudgetDescription, maxWidth: 320)
                }
                LabeledContent("当前生效预算", value: Format.bytes(controller.info?.memoryBudget))
            }


            Section("网络代理") {
                HStack(spacing: 8) {
                    HStack(spacing: 6) {
                        Text("代理模式")
                        InfoTip(text: proxyDescription, maxWidth: 320)
                    }
                    Spacer(minLength: 12)
                    Picker("代理模式", selection: $proxyMode) {
                        ForEach(ProxyMode.allCases) { mode in
                            Text(mode.title).tag(mode.rawValue)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.segmented)
                }

                if proxyMode == ProxyMode.manual.rawValue {
                    TextField("HTTP 代理，例如 127.0.0.1:6152", text: $httpProxy)
                        .textFieldStyle(.roundedBorder)
                    TextField("HTTPS 代理，例如 127.0.0.1:6152", text: $httpsProxy)
                        .textFieldStyle(.roundedBorder)
                }

            }

            Section("模型下载源") {
                HStack(spacing: 8) {
                    HStack(spacing: 6) {
                        Text("下载源")
                        InfoTip(text: downloadSourceDescription, maxWidth: 320)
                    }
                    Spacer(minLength: 12)
                    Picker("下载源", selection: $downloadSource) {
                        ForEach(ModelDownloadSource.allCases) { source in
                            Text(source.title).tag(source.rawValue)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                }

                if selectedDownloadSource == .custom {
                    TextField("例如 https://hf.example.com", text: $customDownloadEndpoint)
                        .textFieldStyle(.roundedBorder)
                    if !customDownloadEndpoint.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                       AppSettings.normalizedDownloadEndpoint(customDownloadEndpoint) == nil {
                        Text("请输入有效的 http/https 地址，不要包含账号、密码、查询参数或片段。")
                            .font(.caption)
                            .foregroundStyle(Theme.danger)
                    }
                }

                LabeledContent("重启后使用", value: downloadEndpointText)
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
        .scrollContentBackground(.hidden)
        .contentMargins(.top, 8, for: .scrollContent)
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

    private var appearanceDescription: String {
        switch AppearanceMode(rawValue: appearance) ?? .system {
        case .system:
            "根据 macOS 当前外观自动切换明亮或暗黑模式。"
        case .light:
            "始终使用明亮外观。"
        case .dark:
            "始终使用暗黑外观。"
        }
    }

    private var memoryBudgetDescription: String {
        "留空使用自动策略：物理内存的 75%，且最多保留 8 GB 给系统。示例：8G、8192M。修改后需要重启 aiworkd 才会生效。"
    }


    private var settingsAreValid: Bool {
        let memoryIsValid = memoryBudget.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            || AppSettings.parseMemoryBudget(memoryBudget) != nil
        let downloadSourceIsValid = selectedDownloadSource != .custom
            || AppSettings.normalizedDownloadEndpoint(customDownloadEndpoint) != nil
        guard proxyMode == ProxyMode.manual.rawValue else {
            return memoryIsValid && downloadSourceIsValid
        }
        let values = [httpProxy, httpsProxy].filter {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
        return memoryIsValid
            && downloadSourceIsValid
            && !values.isEmpty
            && values.allSatisfy { AppSettings.normalizedProxyURL($0) != nil }
    }

    private var selectedDownloadSource: ModelDownloadSource {
        ModelDownloadSource(rawValue: downloadSource) ?? .official
    }

    private var downloadEndpointText: String {
        AppSettings.downloadEndpoint(
            for: selectedDownloadSource,
            customEndpoint: customDownloadEndpoint
        ) ?? "地址无效"
    }

    private var downloadSourceDescription: String {
        switch selectedDownloadSource {
        case .official:
            "适合网络可以稳定访问 Hugging Face 的情况。"
        case .hfMirror:
            "使用 hf-mirror.com；下载中断时会保留 .part 文件并自动续传。"
        case .custom:
            "适合公司内网或自建 Hugging Face 兼容源。切换后点击下方按钮重启 aiworkd 生效。"
        }
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


