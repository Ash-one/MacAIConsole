import Charts
import SwiftUI

struct RuntimeStatusView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(AppRouter.self) private var router
    @State private var selectedModel: LoadedModel?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.sectionSpacing) {
                if let error = controller.lastError {
                    ErrorBanner(text: error, onClose: { controller.lastError = nil })
                }
                headerCard
                statsGrid
                MemoryPressureCard(
                    samples: controller.memorySamples,
                    info: controller.info,
                    phase: controller.phase
                )
                loadedModelsSection
            }
            .padding(Theme.Space.page)
        }
        .navigationTitle("运行状态")
        .sheet(item: $selectedModel) { model in
            RunningModelSettingsView(model: model)
                .environment(controller)
        }
    }

    private var headerCard: some View {
        SectionCard(title: "守护进程", icon: "antenna.radiowaves.left.and.right") {
            HStack(spacing: 16) {
                StatusBadge(phase: controller.phase)
                Spacer(minLength: 0)
                switch controller.phase {
                case .offline:
                    Button("启动 aiworkd") { controller.startDaemon() }
                        .buttonStyle(ProminentButtonStyle())
                case .online:
                    Button("停止", role: .destructive) { controller.stopDaemon() }
                        .buttonStyle(.bordered)
                case .starting, .stopping:
                    HStack(spacing: 8) {
                        ProgressView().controlSize(.small)
                        Text(controller.phase == .starting ? "正在启动…" : "正在停止…")
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
    }

    private var statsGrid: some View {
        let info = controller.info
        return LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 12)], spacing: 12) {
            StatCard(title: "版本", value: info?.version ?? "—", icon: "tag")
            StatCard(title: "PID", value: info.map { "\($0.pid)" } ?? "—", icon: "number")
            StatCard(title: "运行时长", value: info.map { Format.uptime($0.uptimeSecs) } ?? "—", icon: "clock")
            StatCard(title: "活跃请求", value: info.map { "\($0.activeRequests)" } ?? "—", icon: "bolt")
            StatCard(title: "累计任务", value: controller.cumulativeTaskCountText, icon: "list.bullet.rectangle")
            if let budget = info?.memoryBudget, budget > 0 {
            MemoryBudgetCard(
                value: Format.bytes(budget),
                onEdit: { router.page = .settings }
            )
            }
        }
    }

    private var loadedModelsSection: some View {
        SectionCard(title: "Running Models", icon: "cpu") {
            if let models = controller.info?.loadedModels, !models.isEmpty {
                VStack(spacing: 0) {
                    ForEach(models) { model in
                        ModelRow(model: model) {
                            selectedModel = model
                        }
                        if model.id != models.last?.id { HairlineDivider() }
                    }
                }
            } else {
                EmptyHint(
                    text: controller.phase == .online ? "当前没有加载中的模型" : "守护进程离线，暂无数据",
                    systemImage: "cpu"
                )
            }
        }
    }
}

/// 系统内存压力卡片：5 分钟滑动窗口折线 + 折线下方面积填充。
/// 数据来自 DaemonController 在线轮询的采样缓冲，不做逐点滚动动画；
/// 离线断档由时间戳缺口呈现，不插值。
struct MemoryPressureCard: View {
    let samples: [MemoryPressureSample]
    let info: RuntimeInfo?
    let phase: DaemonController.Phase

    private var windowSamples: [MemoryPressureSample] {
        MemoryPressureRecorder.displayWindowSamples(of: samples)
    }

    /// 占用率阈值沿用瞬时压力条时代的变色规则。
    private var lineColor: Color {
        switch windowSamples.last?.usedFraction ?? 0 {
        case ..<0.6: Theme.success
        case ..<0.85: Theme.warning
        default: Theme.danger
        }
    }

    private var xDomain: ClosedRange<Date> {
        let latest = windowSamples.last?.timestamp ?? Date.now
        let first = windowSamples.first?.timestamp ?? latest
        return min(latest.addingTimeInterval(-MemoryPressureRecorder.window), first)...latest
    }

    /// 标题行当前读数：在线时与折线末端同源（同一次刷新）；离线时回退最近样本。
    private var accessoryText: String? {
        if let total = info?.memoryTotal, total > 0, let used = info?.memoryUsed {
            let fraction = min(Double(used) / Double(total), 1)
            return "\(Format.bytes(used)) / \(Format.bytes(total)) · \(fraction.formatted(.percent.precision(.fractionLength(0))))"
        }
        guard let latest = samples.last else { return nil }
        return "最近 \(latest.usedFraction.formatted(.percent.precision(.fractionLength(0))))"
    }

    var body: some View {
        SectionCard(
            title: "系统内存压力",
            icon: "memorychip",
            accessory: {
                if let accessoryText {
                    Text(accessoryText)
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }
        ) {
            if windowSamples.isEmpty {
                EmptyHint(
                    text: phase == .online ? "正在积累内存样本…" : "守护进程离线，暂无数据",
                    systemImage: "chart.xyaxis.line"
                )
            } else {
                chart
                    .frame(height: 160)
            }
        }
    }

    private var chart: some View {
        Chart {
            if windowSamples.count == 1, let only = windowSamples.first {
                PointMark(
                    x: .value("时间", only.timestamp),
                    y: .value("占用率", only.usedFraction)
                )
                .foregroundStyle(lineColor)
            } else {
                ForEach(windowSamples) { sample in
                    AreaMark(
                        x: .value("时间", sample.timestamp),
                        y: .value("占用率", sample.usedFraction)
                    )
                    .foregroundStyle(.linearGradient(
                        colors: [lineColor.opacity(0.32), lineColor.opacity(0.04)],
                        startPoint: .top,
                        endPoint: .bottom
                    ))
                    LineMark(
                        x: .value("时间", sample.timestamp),
                        y: .value("占用率", sample.usedFraction)
                    )
                    .foregroundStyle(lineColor)
                }
            }
        }
        .chartXScale(domain: xDomain)
        .chartYScale(domain: 0...1)
        .chartXAxis {
            AxisMarks(values: .stride(by: .minute)) {
                AxisGridLine()
                AxisValueLabel(format: .dateTime.hour().minute())
            }
        }
        .chartYAxis {
            AxisMarks(values: [0, 0.25, 0.5, 0.75, 1]) { value in
                AxisGridLine()
                if let fraction = value.as(Double.self) {
                    AxisValueLabel {
                        Text("\(Int((fraction * 100).rounded()))%")
                    }
                }
            }
        }
    }
}

struct MemoryBudgetCard: View {
    let value: String
    let onEdit: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 5) {
                Image(systemName: "memorychip")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                Text("内存预算")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 4)
                Button("修改", action: onEdit)
                    .buttonStyle(.borderless)
                    .font(.caption)
            }
            Text(value)
                .font(.title3.weight(.semibold).monospacedDigit())
                .contentTransition(.numericText())
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .themedCard(cornerRadius: 10)
    }
}

struct ModelRow: View {
    @Environment(DaemonController.self) private var controller
    let model: LoadedModel
    let onSelect: () -> Void

    private var modelType: String {
        if let type = model.modelType, !type.isEmpty { type } else { "unknown" }
    }

    var body: some View {
        HStack(spacing: 10) {
            ModelTypeIcon(type: modelType)
            VStack(alignment: .leading, spacing: 3) {
                Text(model.id)
                    .font(.body.weight(.medium))
                HStack(spacing: 6) {
                    if let tag = accelTag {
                        Chip(text: tag.text, color: tag.color)
                    }
                    Text(subtitle)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 12)
            if controller.busyModelIDs.contains(model.id) {
                ProgressView().controlSize(.small)
            } else {
                GhostActionButton(
                    systemImage: "stop.circle",
                    help: "停止模型",
                    activeTint: Theme.danger,
                    isDisabled: controller.phase != .online
                ) {
                    Task { await controller.unload(model.id) }
                }
            }
        }
        .padding(.vertical, 9)
        .padding(.horizontal, 8)
        .hoverableRow()
        .onTapGesture(perform: onSelect)
        .help("点击打开模型设置")
        .contextMenu {
            Button {
                let pasteboard = NSPasteboard.general
                pasteboard.clearContents()
                pasteboard.setString(usageExample, forType: .string)
            } label: {
                Label("拷贝使用示例", systemImage: "doc.on.doc")
            }
            .help("复制调用该模型的 curl 示例")

            Button {
                onSelect()
            } label: {
                Label("详细设置…", systemImage: "slider.horizontal.3")
            }
        }
    }

    /// 按模型类型生成的 curl 调用示例。
    private var usageExample: String {
        let base = controller.api.baseURL.absoluteString.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        switch modelType {
        case "stt":
            // 单引号包裹路径：空格/中文无需反斜杠转义（双引号内 `\ ` 会变成
            // 字面反斜杠，curl 报 26）。若路径本身含单引号，改用双引号并去掉转义。
            return """
            curl \(base)/v1/audio/transcriptions \\
              -F 'file=@你的音频路径.wav' \\
              -F 'model=\(model.id)'
            """
        case "tts":
            return """
            curl \(base)/v1/audio/speech \\
              -H "Content-Type: application/json" \\
              -d '{"model": "\(model.id)", "input": "你好，这是一段试听文本。"}' \\
              -o speech.wav
            """
        default:
            let temperature = model.temperature ?? 1.0
            let topP = model.topP ?? 0.95
            let maxTokens = model.maxTokens ?? 1024
            return """
            curl \(base)/v1/chat/completions \\
              -H "Content-Type: application/json" \\
              -d '{"model": "\(model.id)", "messages": [{"role": "user", "content": "你好"}], "temperature": \(temperature), "top_p": \(topP), "max_tokens": \(maxTokens)}'
            """
        }
    }

    /// 加速策略 tag：仅当检测到 CoreML / Metal 生效时显示。
    private var accelTag: (text: String, color: Color)? {
        switch model.effectiveDevice?.lowercased() {
        case "coreml": ("CoreML", Theme.info)
        case "metal": ("Metal", Theme.warning)
        default: nil
        }
    }

    private var subtitle: String {
        var parts = [model.provider]
        if model.state != "ready" { parts.append("状态：\(model.state)") }
        if let memory = model.memoryUsageBytes, memory > 0 {
            parts.append("驻留内存：\(Format.bytes(memory))")
        } else if modelType == "stt" {
            parts.append("无常驻进程")
        } else {
            parts.append("内存不可测")
        }
        return parts.joined(separator: " · ")
    }
}

struct RunningModelSettingsView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(\.dismiss) private var dismiss
    let model: LoadedModel

    @State private var keepAlive: String
    @State private var contextLengthDraft: String
    @State private var repoModel: RepoModel?
    @State private var isReloading = false

    private struct KeepAliveChoice: Identifiable {
        let label: String
        let value: String
        var id: String { value }
    }

    private let keepAliveChoices = [
        KeepAliveChoice(label: "1min", value: "1m"),
        KeepAliveChoice(label: "5min", value: "5m"),
        KeepAliveChoice(label: "10min", value: "10m"),
        KeepAliveChoice(label: "30min", value: "30m"),
        KeepAliveChoice(label: "60min", value: "60m"),
        KeepAliveChoice(label: "120min", value: "120m"),
        KeepAliveChoice(label: "始终", value: "always"),
    ]

    init(model: LoadedModel) {
        self.model = model
        let contextLength = model.contextLength ?? ModelRepository.contextLength(for: model.id)
        _keepAlive = State(initialValue: model.keepAlive?.isEmpty == false ? model.keepAlive! : "always")
        _contextLengthDraft = State(initialValue: Self.displayContextLength(contextLength))
    }

    private var modelType: String {
        if let type = model.modelType, !type.isEmpty { type } else { "unknown" }
    }

    private var typeLabel: String {
        switch modelType {
        case "llm": "LLM"
        case "stt": "STT"
        case "tts": "TTS"
        default: modelType.uppercased()
        }
    }

    private static func displayContextLength(_ value: Int) -> String {
        let kilo = Double(value) / 1024.0
        if kilo.rounded() == kilo { return String(Int(kilo)) }
        return String(format: "%.2f", kilo)
    }

    private static func parseContextLength(_ text: String) -> Int? {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard let kilo = Double(trimmed), kilo > 0, kilo <= 1024 else { return nil }
        return Int(kilo * 1024.0)
    }

    private var parsedContextLength: Int? {
        Self.parseContextLength(contextLengthDraft)
    }

    private var currentContextLength: Int {
        model.contextLength ?? ModelRepository.contextLength(for: model.id)
    }

    private var contextLengthChanged: Bool {
        parsedContextLength != nil && parsedContextLength != currentContextLength
    }

    private var contextLengthValid: Bool {
        guard let value = parsedContextLength else { return false }
        let detectedLimit = repoModel?.ggufMetadata?.contextLength ?? 1024 * 1024
        return value >= 256 && value <= min(detectedLimit, 1024 * 1024)
    }

    private var contextDescription: String {
        guard let repoModel else {
            return "该模型不在模型仓库中，无法从这里重载上下文"
        }
        if let detectionError = repoModel.detectionError {
            return "GGUF 检测失败：\(detectionError)"
        }
        if let contextLength = repoModel.ggufMetadata?.contextLength {
            return "模型原生上限 \(GGUFMetadata.formatTokenCount(contextLength))；修改后会同步注册设置并重载 LLM"
        }
        return "修改后会同步模型注册设置并重载 LLM"
    }

    private var canApply: Bool {
        guard controller.phase == .online, !isReloading else { return false }
        guard modelType == "llm" else { return true }
        guard contextLengthChanged else { return true }
        return contextLengthValid && repoModel?.detectionError == nil
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            Form {
                Section("运行策略") {
                    Picker("保持时间", selection: $keepAlive) {
                        ForEach(keepAliveChoices) { choice in
                            Text(choice.label).tag(choice.value)
                        }
                    }
                    .pickerStyle(.menu)
                    .onChange(of: keepAlive) { _, value in
                        Task { await controller.setKeepAlive(model.id, keepAlive: value) }
                    }
                }

                if modelType == "llm" {
                    Section("推理") {
                        HStack {
                            Text("上下文长度")
                            Spacer()
                            HStack(spacing: 4) {
                                TextField(
                                    "上下文长度",
                                    text: $contextLengthDraft,
                                    prompt: Text("4")
                                )
                                    .labelsHidden()
                                    .font(.body.monospacedDigit())
                                    .textFieldStyle(.plain)
                                    .frame(width: 72)
                                    .multilineTextAlignment(.trailing)
                                    .onChange(of: contextLengthDraft) { _, value in
                                        let filtered = value.filter { $0.isNumber || $0 == "." }
                                        if filtered != value { contextLengthDraft = filtered }
                                    }
                                Text("K")
                                    .font(.body.weight(.medium).monospacedDigit())
                                    .foregroundStyle(.secondary)
                            }
                            .padding(.horizontal, 9)
                            .padding(.vertical, 6)
                            .background(
                                RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                                    .fill(Theme.inset)
                            )
                            .overlay(
                                RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                                    .strokeBorder(Theme.hairlineStrong)
                            )
                        }
                        Text(contextDescription)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }

                if modelType == "tts" {
                    Section("语音") {
                        VoiceSelectionControl(modelID: model.id)
                    }
                }
            }
            .formStyle(.grouped)
        }
        .frame(minWidth: 420, minHeight: modelType == "tts" ? 300 : 250)
        .navigationTitle("模型设置")
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                Button(isReloading ? "重载中…" : "应用") {
                    applySettings()
                }
                .disabled(!canApply)
            }
        }
        .task(id: model.id) {
            repoModel = await ModelRepository.scanInBackground().first { $0.modelID == model.id }
        }
    }

    private func applySettings() {
        guard modelType == "llm", contextLengthChanged else {
            dismiss()
            return
        }
        applyContextLength()
    }

    private func applyContextLength() {
        guard modelType == "llm",
              let value = parsedContextLength,
              contextLengthValid,
              let repoModel,
              repoModel.detectionError == nil,
              !isReloading else { return }

        ModelRepository.setContextLength(value, for: model.id)
        isReloading = true
        Task {
            do {
                try await controller.registerAndLoad(
                    path: repoModel.path,
                    id: model.id,
                    contextLength: value,
                    keepAlive: keepAlive,
                    modelType: nil,
                    provider: model.provider
                )
            } catch {
                controller.lastError = "上下文重载失败：\(DaemonController.message(for: error))"
            }
            isReloading = false
            dismiss()
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 12) {
            ModelTypeIcon(type: modelType)
            VStack(alignment: .leading, spacing: 4) {
                Text(model.id)
                    .font(.title2.weight(.semibold))
                Text("\(typeLabel) · \(model.provider)")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                HStack(spacing: 6) {
                    StatusDot(
                        color: model.state == "ready" ? Theme.success : Theme.warning,
                        glow: model.state == "ready"
                    )
                    Text(model.state == "ready" ? "运行中" : model.state)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(model.state == "ready" ? Theme.success : Theme.warning)
                }
            }
            Spacer()
        }
        .padding(20)
    }
}
