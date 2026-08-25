import SwiftUI

struct RuntimeStatusView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(\.openSettings) private var openSettings
    @State private var selectedModel: LoadedModel?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let error = controller.lastError {
                    ErrorBanner(text: error)
                }
                headerCard
                statsGrid
                loadedModelsSection
                providerSection
            }
            .padding(20)
        }
        .navigationTitle("运行状态")
        .sheet(item: $selectedModel) { model in
            RunningModelSettingsView(model: model)
                .environment(controller)
        }
    }

    private var headerCard: some View {
        GroupBox {
            HStack(spacing: 16) {
                StatusBadge(phase: controller.phase)
                Spacer(minLength: 0)
                switch controller.phase {
                case .offline:
                    Button("启动 aiworkd") { controller.startDaemon() }
                        .buttonStyle(.borderedProminent)
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
            .padding(4)
        }
    }

    private var statsGrid: some View {
        let info = controller.info
        return LazyVGrid(columns: [GridItem(.adaptive(minimum: 130), spacing: 12)], spacing: 12) {
            StatCard(title: "版本", value: info?.version ?? "—")
            StatCard(title: "PID", value: info.map { "\($0.pid)" } ?? "—")
            StatCard(title: "运行时长", value: info.map { Format.uptime($0.uptimeSecs) } ?? "—")
            StatCard(title: "活跃请求", value: info.map { "\($0.activeRequests)" } ?? "—")
            if let budget = info?.memoryBudget, budget > 0 {
            MemoryBudgetCard(
                value: Format.bytes(budget),
                onEdit: { openSettings() }
            )
            }
        }
    }

    private var loadedModelsSection: some View {
        GroupBox {
            if let models = controller.info?.loadedModels, !models.isEmpty {
                VStack(spacing: 0) {
                    ForEach(models) { model in
                        ModelRow(model: model) {
                            selectedModel = model
                        }
                        if model.id != models.last?.id { Divider() }
                    }
                }
                .padding(.horizontal, 4)
            } else {
                Text(controller.phase == .online ? "当前没有加载中的模型" : "守护进程离线，暂无数据")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 44)
            }
        } label: {
            Text("Running Models")
                .font(.title3.weight(.semibold))
                .padding(.bottom, 8)
        }
    }

    private var providerSection: some View {
        GroupBox("Provider 状态") {
            if controller.providers.isEmpty {
                Text("暂无 Provider 数据")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 44)
            } else {
                VStack(spacing: 0) {
                    ForEach(controller.providers) { provider in
                        ProviderRow(entry: provider)
                        if provider.id != controller.providers.last?.id { Divider() }
                    }
                }
                .padding(.horizontal, 4)
            }
        }
    }
}

struct MemoryBudgetCard: View {
    let value: String
    let onEdit: () -> Void

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
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
            .padding(2)
        }
    }
}

struct ModelRow: View {
    @Environment(DaemonController.self) private var controller
    let model: LoadedModel
    let onSelect: () -> Void

    @State private var isHovering = false

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(model.id)
                    .font(.body.weight(.medium))
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 12)
            if controller.busyModelIDs.contains(model.id) {
                ProgressView().controlSize(.small)
            } else {
                Button("卸载") {
                    Task { await controller.unload(model.id) }
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(controller.phase != .online)
            }
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 6)
        .contentShape(Rectangle())
        .background(isHovering ? Color.primary.opacity(0.05) : Color.clear)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .onHover { isHovering = $0 }
        .animation(.snappy(duration: 0.15), value: isHovering)
        .onTapGesture(perform: onSelect)
        .help("点击打开模型设置")
    }

    private var subtitle: String {
        var parts = [model.provider]
        if model.state != "ready" { parts.append("状态：\(model.state)") }
        if let memory = model.memoryEstimate, memory > 0 { parts.append(Format.bytes(memory)) }
        parts.append("\(Format.relativeTime(model.loadedAt))加载")
        return parts.joined(separator: " · ")
    }
}

struct RunningModelSettingsView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(\.dismiss) private var dismiss
    let model: LoadedModel

    @State private var keepAlive: String
    @State private var voice: String
    @State private var contextLengthDraft: String
    @State private var repoModel: RepoModel?
    @State private var isReloading = false
    @State private var voices: [String] = []
    @State private var voicesLoaded = false

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
        _voice = State(initialValue: model.defaultVoice?.isEmpty == false ? model.defaultVoice! : "zf_001")
        _contextLengthDraft = State(initialValue: Self.displayContextLength(contextLength))
    }

    private var modelType: String {
        if let type = model.modelType, !type.isEmpty { return type }
        if model.provider == "kokoro-mlx" { return "tts" }
        if model.provider == "whisper.cpp" { return "stt" }
        return "llm"
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
        return value >= 256 && value <= 1024 * 1024
    }

    private var canApply: Bool {
        guard controller.phase == .online, !isReloading else { return false }
        guard modelType == "llm" else { return true }
        return !contextLengthChanged || (contextLengthValid && repoModel != nil)
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
                                TextField("4", text: $contextLengthDraft)
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
                                RoundedRectangle(cornerRadius: 7)
                                    .fill(Color.secondary.opacity(0.12))
                            )
                            .overlay(
                                RoundedRectangle(cornerRadius: 7)
                                    .strokeBorder(Color.secondary.opacity(0.18))
                            )
                        }
                        Text(repoModel == nil ? "该模型不在模型仓库中，无法从这里重载上下文" : "修改后会同步模型注册设置并重载 LLM")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }

                if modelType == "tts" {
                    Section("语音") {
                        if voices.isEmpty {
                            HStack {
                                Text(voicesLoaded ? "未找到可用音色" : "正在读取音色…")
                                    .foregroundStyle(.secondary)
                                if !voicesLoaded { ProgressView().controlSize(.small) }
                            }
                        } else {
                            Picker("默认音色", selection: $voice) {
                                ForEach(voices, id: \.self) { item in
                                    Text(item).tag(item)
                                }
                            }
                            .pickerStyle(.menu)
                            .onChange(of: voice) { _, value in
                                Task { await controller.setVoice(model.id, voice: value) }
                            }
                        }
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
            repoModel = ModelRepository.scan().first { $0.modelID == model.id }
            guard modelType == "tts", !voicesLoaded else { return }
            defer { voicesLoaded = true }
            if let response = try? await controller.voices(for: model.id) {
                voices = response.voices
                if let defaultVoice = response.defaultVoice, !defaultVoice.isEmpty {
                    voice = defaultVoice
                }
            }
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
                    modelType: nil
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
                    Circle()
                        .fill(model.state == "ready" ? Color.green : Color.orange)
                        .frame(width: 7, height: 7)
                    Text(model.state == "ready" ? "运行中" : model.state)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(model.state == "ready" ? .green : .orange)
                }
            }
            Spacer()
        }
        .padding(20)
    }
}

struct ProviderRow: View {
    let entry: ProviderEntry

    var body: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(entry.descriptor.id)
                        .font(.body.weight(.medium))
                    if let isolation = entry.descriptor.isolation, !isolation.isEmpty {
                        Text(isolation)
                            .font(.caption2)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Capsule().fill(.secondary.opacity(0.12)))
                            .foregroundStyle(.secondary)
                    }
                }
                if let capabilities = entry.descriptor.capabilities, !capabilities.isEmpty {
                    Text(capabilities.joined(separator: "、"))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if let reason = entry.status.reason, !reason.isEmpty {
                    Text(reason)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 12)
            HStack(spacing: 4) {
                Circle()
                    .fill(entry.status.available ? Color.green : Color.red)
                    .frame(width: 8, height: 8)
                Text(statusText)
                    .font(.caption.weight(.medium))
                    .lineLimit(1)
            }
            .fixedSize()
        }
        .padding(.vertical, 6)
    }

    private var statusText: String {
        var text = entry.status.ready ? "就绪" : (entry.status.available ? "可用" : "不可用")
        if let device = entry.status.effectiveDevice, !device.isEmpty {
            text += " · \(device)"
        }
        return text
    }
}