import Charts
import SwiftUI

struct RuntimeStatusView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(AppRouter.self) private var router
    @State private var selectedTarget: ModelDetailTarget?
    @State private var inspections: [String: LocalInspection] = [:]

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
        .sheet(item: $selectedTarget) { target in
            ModelDetailSheet(
                target: target,
                inspection: inspection(for: target),
                onUpdate: {
                    try? await controller.refresh()
                }
            )
            .environment(controller)
            .environment(router)
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
                onEdit: { router.goToSettings() }
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
                            openDetail(for: model)
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

    private func inspection(for target: ModelDetailTarget) -> LocalInspection? {
        switch target {
        case .registered(let entry):
            guard let path = entry.path else { return nil }
            return inspections[path]
        case .repo(let model):
            return inspections[model.path]
        case .profile:
            return nil
        }
    }

    private func target(for model: LoadedModel) -> ModelDetailTarget {
        if let entry = controller.registeredModels.first(where: { $0.id == model.id }) {
            return .registered(entry)
        }
        let fallback = ModelEntry(
            id: model.id,
            ownedBy: model.provider,
            requestedProvider: nil,
            providerSelectionReason: nil,
            modelType: model.modelType ?? "llm",
            path: nil,
            temperature: model.temperature,
            topP: model.topP,
            maxTokens: model.maxTokens
        )
        return .registered(fallback)
    }

    private func openDetail(for model: LoadedModel) {
        let t = target(for: model)
        selectedTarget = t
        if case .registered(let entry) = t, let path = entry.path, inspections[path] == nil {
            Task {
                if let result = try? await controller.api.inspectModels(paths: [path]), let first = result.first {
                    inspections[path] = first
                }
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

