import AppKit
import SwiftUI

/// 菜单栏小窗：现代化状态看板 + 统一内存监控 + 常驻模型管理 + 快速加载 + 深度快捷导航。
struct MenuBarContentView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(AppRouter.self) private var router
    @Environment(\.openWindow) private var openWindow

    @State private var copiedEndpoint = false

    private var baseURLString: String {
        controller.api.baseURL.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
    }

    private var unloadedModels: [ModelEntry] {
        let loadedIDs = Set(controller.info?.loadedModels.map(\.id) ?? [])
        return controller.registeredModels.filter { !loadedIDs.contains($0.id) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            headerSection

            if controller.phase == .online {
                endpointBanner
                if let info = controller.info {
                    memoryAndMetricsCard(info: info)
                }
                if !controller.runningTasks.isEmpty {
                    runningTasksSection
                }
                loadedModelsSection
                quickLoadSection
            } else if controller.phase == .offline {
                offlineStateSection
            } else {
                connectingStateSection
            }

            HairlineDivider()

            daemonControlsSection

            navigationSection

            HairlineDivider()

            footerSection
        }
        .padding(12)
        .frame(width: 340)
    }

    // MARK: - 头部品牌与状态

    private var headerSection: some View {
        HStack(spacing: 9) {
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(Theme.accentGradient)
                .frame(width: 26, height: 26)
                .overlay {
                    Image(systemName: "cpu.fill")
                        .font(.system(size: 13, weight: .bold))
                        .foregroundStyle(.black.opacity(0.85))
                }

            VStack(alignment: .leading, spacing: 1) {
                HStack(spacing: 4) {
                    Text("MacAI")
                        .font(.headline)
                    Text("Console")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                if let version = controller.info?.version {
                    Text("v\(version)")
                        .font(.caption2.monospaced())
                        .foregroundStyle(.tertiary)
                }
            }

            Spacer(minLength: 4)

            StatusBadge(phase: controller.phase)

            Button {
                navigateTo(.runtime)
            } label: {
                Image(systemName: "macwindow")
                    .font(.callout.weight(.medium))
                    .foregroundStyle(.secondary)
                    .frame(width: 24, height: 24)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("打开主窗口 (⌘O)")
            .keyboardShortcut("o")
        }
    }

    // MARK: - API 端点与连通提示

    private var endpointBanner: some View {
        HStack(spacing: 8) {
            Image(systemName: "network")
                .font(.caption2)
                .foregroundStyle(Theme.accent)

            Text(baseURLString)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)

            Spacer(minLength: 4)

            if let active = controller.info?.activeRequests, active > 0 {
                Chip(text: "\(active) 进行中", color: Theme.warning)
            }

            Button {
                let pasteboard = NSPasteboard.general
                pasteboard.clearContents()
                pasteboard.setString(baseURLString, forType: .string)
                withAnimation(.snappy) { copiedEndpoint = true }
                Task {
                    try? await Task.sleep(for: .seconds(1.8))
                    withAnimation(.snappy) { copiedEndpoint = false }
                }
            } label: {
                HStack(spacing: 3) {
                    Image(systemName: copiedEndpoint ? "checkmark" : "doc.on.doc")
                        .font(.caption2)
                    if copiedEndpoint {
                        Text("已复制")
                            .font(.caption2)
                    }
                }
                .foregroundStyle(copiedEndpoint ? Theme.success : .secondary)
                .padding(.horizontal, 6)
                .padding(.vertical, 3)
                .background(
                    RoundedRectangle(cornerRadius: 4, style: .continuous)
                        .fill(copiedEndpoint ? Theme.success.opacity(0.12) : Color.primary.opacity(0.06))
                )
            }
            .buttonStyle(.plain)
            .help("复制 API 端点地址")
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 6)
        .background(
            RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                .fill(Theme.inset)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                .strokeBorder(Theme.hairline)
        )
    }

    // MARK: - 统一内存与指标仪表

    private func memoryAndMetricsCard(info: RuntimeInfo) -> some View {
        VStack(spacing: 8) {
            if let total = info.memoryTotal, total > 0, let used = info.memoryUsed {
                let fraction = min(Double(used) / Double(total), 1.0)
                let percentText = "\((fraction * 100).formatted(.number.precision(.fractionLength(0))))%"
                let barColor = fraction < 0.6 ? Theme.success : fraction < 0.85 ? Theme.warning : Theme.danger

                VStack(spacing: 4) {
                    HStack {
                        Label("统一内存占用", systemImage: "memorychip")
                            .font(.caption.weight(.medium))
                            .foregroundStyle(.secondary)
                        Spacer()
                        Text("\(Format.bytes(used)) / \(Format.bytes(total)) · \(percentText)")
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.primary)
                    }

                    GeometryReader { geo in
                        ZStack(alignment: .leading) {
                            Capsule()
                                .fill(Color.primary.opacity(0.08))
                                .frame(height: 6)

                            Capsule()
                                .fill(barColor)
                                .frame(width: max(geo.size.width * CGFloat(fraction), 6), height: 6)
                        }
                    }
                    .frame(height: 6)

                    if let budget = info.memoryBudget, budget > 0 {
                        HStack {
                            Spacer()
                            Text("AI 调度预算：\(Format.bytes(budget))")
                                .font(.caption2)
                                .foregroundStyle(.tertiary)
                        }
                    }
                }
            }

            HStack(spacing: 8) {
                metricPill(title: "活跃请求", value: "\(info.activeRequests)", icon: "bolt.fill", highlight: info.activeRequests > 0)
                metricPill(title: "运行时长", value: Format.uptime(info.uptimeSecs), icon: "clock")
                metricPill(title: "累计任务", value: controller.cumulativeTaskCountText, icon: "list.bullet.rectangle")
            }
        }
        .padding(10)
        .themedCard(cornerRadius: 10)
    }

    private func metricPill(title: String, value: String, icon: String, highlight: Bool = false) -> some View {
        VStack(spacing: 2) {
            HStack(spacing: 3) {
                Image(systemName: icon)
                    .font(.caption2)
                    .foregroundStyle(highlight ? Theme.warning : Color.primary.opacity(0.45))
                Text(title)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Text(value)
                .font(.callout.weight(.semibold).monospacedDigit())
                .foregroundStyle(highlight ? Theme.warning : .primary)
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 5)
        .background(
            RoundedRectangle(cornerRadius: 6, style: .continuous)
                .fill(Color.primary.opacity(0.04))
        )
    }

    // MARK: - 活跃任务感知

    @ViewBuilder
    private var runningTasksSection: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack {
                Label("正在推理 (\(controller.runningTasks.count))", systemImage: "bolt.circle.fill")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(Theme.warning)
                Spacer()
            }

            ForEach(controller.runningTasks) { task in
                HStack(spacing: 6) {
                    ProgressView().controlSize(.mini)
                    Text(task.model)
                        .font(.caption.weight(.medium))
                        .lineLimit(1)
                    Spacer()
                    Text(Format.runningDuration(startedAtMs: task.startedAtMs))
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 4)
                .background(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(Theme.warning.opacity(0.08))
                )
            }
        }
    }

    // MARK: - 常驻模型管理

    private var loadedModelsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("常驻模型")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)

                if let count = controller.info?.loadedModels.count {
                    Chip(text: "\(count)", color: count > 0 ? Theme.success : nil)
                }

                Spacer()

                if let models = controller.info?.loadedModels, !models.isEmpty {
                    Text("右键拷贝示例")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
            }

            if let models = controller.info?.loadedModels, !models.isEmpty {
                VStack(spacing: 4) {
                    ForEach(models) { model in
                        loadedModelRow(model)
                    }
                }
            } else {
                HStack {
                    Spacer()
                    Text("暂无模型常驻内存")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                    Spacer()
                }
                .padding(.vertical, 8)
                .background(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(Color.primary.opacity(0.03))
                )
            }
        }
    }

    private func loadedModelRow(_ model: LoadedModel) -> some View {
        HStack(spacing: 8) {
            ModelTypeIcon(type: model.modelType ?? "llm")

            VStack(alignment: .leading, spacing: 2) {
                Text(model.id)
                    .font(.caption.weight(.semibold))
                    .lineLimit(1)
                    .truncationMode(.middle)

                HStack(spacing: 4) {
                    if let dev = model.effectiveDevice?.lowercased() {
                        if dev == "metal" {
                            Chip(text: "Metal", color: Theme.warning)
                        } else if dev == "coreml" {
                            Chip(text: "CoreML", color: Theme.info)
                        }
                    }
                    if let mem = model.memoryUsageBytes, mem > 0 {
                        Text(Format.bytes(mem))
                            .font(.caption2.monospacedDigit())
                            .foregroundStyle(.secondary)
                    } else if model.modelType == "stt" {
                        Text("无常驻进程")
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                    }
                }
            }

            Spacer(minLength: 6)

            if controller.busyModelIDs.contains(model.id) {
                ProgressView().controlSize(.small)
            } else {
                GhostActionButton(
                    systemImage: "stop.circle",
                    help: "卸载常驻模型",
                    activeTint: Theme.danger,
                    isDisabled: controller.phase != .online
                ) {
                    Task { await controller.unload(model.id) }
                }
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .hoverableRow(cornerRadius: 6)
        .contextMenu {
            Button {
                let pasteboard = NSPasteboard.general
                pasteboard.clearContents()
                pasteboard.setString(model.id, forType: .string)
            } label: {
                Label("拷贝模型名称", systemImage: "doc.on.doc")
            }

            Button {
                let pasteboard = NSPasteboard.general
                pasteboard.clearContents()
                pasteboard.setString(usageExample(for: model), forType: .string)
            } label: {
                Label("拷贝 curl 调用示例", systemImage: "terminal")
            }

            Divider()

            Button(role: .destructive) {
                Task { await controller.unload(model.id) }
            } label: {
                Label("卸载模型", systemImage: "stop.circle")
            }
        }
    }

    // MARK: - 快捷拉起未加载模型

    @ViewBuilder
    private var quickLoadSection: some View {
        let unloaded = unloadedModels
        if !unloaded.isEmpty {
            Menu {
                ForEach(unloaded) { entry in
                    Button {
                        Task { await controller.loadRegistered(entry.id) }
                    } label: {
                        Label(entry.id, systemImage: modelIconName(for: entry.modelType))
                    }
                }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "plus.circle")
                        .foregroundStyle(Theme.accent)
                    Text("快速加载本地模型…")
                        .foregroundStyle(.primary)
                    Spacer()
                    Text("\(unloaded.count) 个可用")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                    Image(systemName: "chevron.up.chevron.down")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
                .font(.caption.weight(.medium))
                .padding(.horizontal, 9)
                .padding(.vertical, 6)
                .background(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill(Color.primary.opacity(0.04))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .strokeBorder(Theme.hairline)
                )
            }
            .menuStyle(.borderlessButton)
        }
    }

    // MARK: - 离线与连接中状态

    private var offlineStateSection: some View {
        VStack(spacing: 6) {
            Image(systemName: "poweroff")
                .font(.title2)
                .foregroundStyle(.tertiary)
            Text("aiworkd 未运行")
                .font(.callout.weight(.medium))
            Text("本地推理 API 处于离线状态，点击下方按钮拉起守护进程。")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 14)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(Color.primary.opacity(0.03))
        )
    }

    private var connectingStateSection: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(controller.phase == .starting ? "正在启动守护进程…" : "正在连接守护进程…")
                .font(.callout)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 12)
    }

    // MARK: - 守护进程控制

    private var daemonControlsSection: some View {
        HStack(spacing: 8) {
            switch controller.phase {
            case .online:
                Button {
                    controller.restartDaemon()
                } label: {
                    Label("重启 aiworkd", systemImage: "arrow.clockwise")
                }
                .buttonStyle(.bordered)
                .controlSize(.small)

                Spacer()

                Button(role: .destructive) {
                    controller.stopDaemon()
                } label: {
                    Label("停止", systemImage: "stop.circle")
                }
                .buttonStyle(.bordered)
                .controlSize(.small)

            case .offline:
                Button {
                    controller.startDaemon()
                } label: {
                    Label("启动 aiworkd", systemImage: "play.circle.fill")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(ProminentButtonStyle())
                .controlSize(.small)

            case .starting, .stopping:
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text(controller.phase == .starting ? "正在启动守护进程…" : "正在停止守护进程…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .center)
            }
        }
    }

    // MARK: - 快捷导航栏

    private var navigationSection: some View {
        HStack(spacing: 6) {
            navButton(title: "状态", icon: "gauge", page: .runtime)
            navButton(title: "任务", icon: "list.bullet.rectangle.portrait", page: .tasks)
            navButton(title: "管理", icon: "shippingbox", page: .models)
            navButton(title: "日志", icon: "doc.text.magnifyingglass", page: .logs)
            navButton(title: "设置", icon: "gearshape", page: .settings)
        }
    }

    private func navButton(title: String, icon: String, page: Page) -> some View {
        Button {
            navigateTo(page)
        } label: {
            VStack(spacing: 2) {
                Image(systemName: icon)
                    .font(.footnote)
                Text(title)
                    .font(.caption2)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 4)
            .foregroundStyle(.secondary)
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(Color.primary.opacity(0.04))
            )
        }
        .buttonStyle(.plain)
        .hoverableRow(cornerRadius: 6)
        .help("前往「\(title)」页面")
    }

    // MARK: - 底部操作栏

    private var footerSection: some View {
        HStack {
            Button("打开主控制台") {
                navigateTo(.runtime)
            }
            .buttonStyle(.borderless)
            .font(.caption)

            Spacer()

            Button("退出") {
                NSApp.terminate(nil)
            }
            .buttonStyle(.borderless)
            .font(.caption)
            .foregroundStyle(.secondary)
            .keyboardShortcut("q")
        }
    }

    // MARK: - 辅助方法

    private func navigateTo(_ page: Page) {
        router.page = page
        if let mainWindow = NSApp.windows.first(where: { $0.canBecomeMain && !($0 is NSPanel) && ($0.isVisible || $0.isMiniaturized) }) {
            if mainWindow.isMiniaturized {
                mainWindow.deminiaturize(nil)
            }
            mainWindow.makeKeyAndOrderFront(nil)
        } else {
            openWindow(id: "main")
        }
        NSApp.activate(ignoringOtherApps: true)
    }

    private func modelIconName(for type: String) -> String {
        switch type {
        case "stt": "waveform"
        case "tts": "speaker.wave.2"
        default: "brain"
        }
    }

    private func usageExample(for model: LoadedModel) -> String {
        switch model.modelType ?? "llm" {
        case "stt":
            return """
            curl \(baseURLString)/v1/audio/transcriptions \\
              -F 'file=@你的音频路径.wav' \\
              -F 'model=\(model.id)'
            """
        case "tts":
            return """
            curl \(baseURLString)/v1/audio/speech \\
              -H "Content-Type: application/json" \\
              -d '{"model": "\(model.id)", "input": "你好，这是一段试听文本。"}' \\
              -o speech.wav
            """
        default:
            let temperature = model.temperature ?? 1.0
            let topP = model.topP ?? 0.95
            let maxTokens = model.maxTokens ?? 1024
            return """
            curl \(baseURLString)/v1/chat/completions \\
              -H "Content-Type: application/json" \\
              -d '{"model": "\(model.id)", "messages": [{"role": "user", "content": "你好"}], "temperature": \(temperature), "top_p": \(topP), "max_tokens": \(maxTokens)}'
            """
        }
    }
}