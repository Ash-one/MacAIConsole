import AppKit
import SwiftUI

/// 模型详细设置的目标枚举：支持模型注册 ID（已注册）、模型仓库（本地扫描）与推荐模型。
enum ModelDetailTarget: Identifiable, Hashable {
    case registered(ModelEntry)
    case repo(RepoModel)
    case profile(ModelProfile)

    var id: String {
        switch self {
        case .registered(let entry): "registered-\(entry.id)"
        case .repo(let model): "repo-\(model.path)"
        case .profile(let model): "profile-\(model.id)"
        }
    }
}

/// 模型详细设置弹窗：展示模型的 Runner 识别结果、识别依据、运行规格、文件路径与参数设置。
struct ModelDetailSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(DaemonController.self) private var controller
    @Environment(AppRouter.self) private var router

    let target: ModelDetailTarget
    let inspection: LocalInspection?
    let onUpdate: () async -> Void

    // 上下文长度编辑状态（针对 LLM）
    @State private var contextDraft = ""
    @State private var isApplyingContext = false
    @State private var contextFeedback: String?
    @State private var temperatureDraft = 1.0
    @State private var topPDraft = 0.95
    @State private var maxTokensDraft: UInt64 = 1024
    @State private var isApplyingGeneration = false
    @State private var generationFeedback: String?

    // 注册 ID 更改状态（针对已注册模型）
    @State private var idDraft = ""
    @State private var isRenaming = false
    @State private var renameError: String?

    // 操作状态
    @State private var isPerformingAction = false
    @State private var actionError: String?
    @State private var isPreviewingTTS = false
    @State private var copiedPath = false

    // MARK: - 计算属性与数据映射

    private var modelID: String {
        switch target {
        case .registered(let entry): entry.id
        case .repo(let model): model.modelID
        case .profile(let profile): profile.id
        }
    }

    private var titleText: String {
        switch target {
        case .registered(let entry): entry.id
        case .repo(let model): model.fileName
        case .profile(let profile): profile.name
        }
    }

    private var modelType: String {
        switch target {
        case .registered(let entry): entry.modelType
        case .repo(let model): model.modelType
        case .profile(let profile): profile.modelType
        }
    }

    private var modelTypeLabel: String {
        switch modelType {
        case "llm": "对话模型 · LLM"
        case "stt": "语音识别 · STT"
        case "tts": "语音合成 · TTS"
        default: "其他 · \(modelType)"
        }
    }

    private var filePath: String? {
        switch target {
        case .registered(let entry): entry.path
        case .repo(let model): model.path
        case .profile(let profile): profile.isDownloaded ? profile.localURL.path : nil
        }
    }

    private var loadedModel: LoadedModel? {
        controller.info?.loadedModels.first { $0.id == modelID }
    }

    private var isLoaded: Bool {
        loadedModel != nil
    }

    private var isWorker: Bool {
        guard let runnerID else { return false }
        return controller.providers.first { $0.descriptor.id == runnerID }?
            .descriptor.isolation == "worker"
    }

    // MARK: - Runner 及识别依据推导

    private var runnerID: String? {
        switch target {
        case .registered(let entry):
            return entry.ownedBy.split(separator: "/").last.map(String.init) ?? entry.ownedBy
        case .repo(let model):
            if model.isDirectory {
                if let inspection, inspection.status == "recognized" {
                    return inspection.matches.first?.runner
                }
                return nil
            }
            if model.modelType == "llm" {
                return "org.macai.llama.cpp"
            }
            if model.modelType == "stt" {
                return "org.macai.whisper.cpp"
            }
            return nil
        case .profile(let profile):
            return profile.runner
        }
    }

    private var runnerEntry: ProviderEntry? {
        guard let runnerID else { return nil }
        return controller.providers.first { $0.descriptor.id == runnerID }
    }

    private var runnerIsAvailable: Bool {
        runnerEntry?.status.available ?? false
    }

    /// Runner 识别依据与判定机制说明
    private var runnerRecognitionInfo: (method: String, detail: String, isSuccess: Bool) {
        switch target {
        case .registered(let entry):
            let method = "注册时由 daemon 裁决路由"
            var details: [String] = []
            if let reason = entry.providerSelectionReason {
                details.append("裁决依据：\(reason)")
            }
            if let requested = entry.requestedProvider {
                details.append("请求来源：\(requested)")
            }
            if details.isEmpty {
                details.append("已在 daemon 注册表中持久化绑定")
            }
            return (method, details.joined(separator: " · "), true)

        case .repo(let model):
            if model.isDirectory {
                if let inspection {
                    switch inspection.status {
                    case "recognized":
                        if let match = inspection.matches.first {
                            let detail = "探测器 [\(match.detectorID)] 匹配规则：\(match.reason)（适配器: \(match.adapter)，能力: \(match.capability)）"
                            return ("本地目录探测器（Local Detector）", detail, true)
                        }
                        return ("本地目录探测器", "已识别匹配的 Runner 签名", true)
                    case "ambiguous":
                        let runners = inspection.matches.map(\.runner).joined(separator: ", ")
                        return ("多重匹配歧义（Ambiguous）", "匹配到多个 Runner（\(runners)），需显式指定 provider 才能加载", false)
                    case "unsupported":
                        let diag = inspection.diagnostics.first ?? "未匹配到任何已安装 Runner 的目录探测规则"
                        return ("未受支持的目录（Unsupported）", diag, false)
                    default:
                        return ("目录检测状态：\(inspection.status)", inspection.diagnostics.first ?? "暂无诊断信息", false)
                    }
                }
                return ("目录待检测", "尚未完成目录探测扫描", false)
            }
            if model.modelType == "llm" {
                return (
                    "GGUF 二进制文件特征识别",
                    "基于 .gguf 扩展名与 GGUF Magic Header 文件签名解析，由 llama.cpp Runner 自动承载",
                    model.detectionError == nil
                )
            }
            if model.modelType == "stt" {
                return (
                    "Whisper 权重文件识别",
                    "基于 Models/stt/ 路径与 .bin 权重文件扩展名检测，由 whisper.cpp Runner 自动承载",
                    true
                )
            }
            return ("未知文件类型", "未能从文件结构中识别适用的 Runner", false)

        case .profile(let profile):
            return (
                "官方内置 Model Profile 绑定",
                "由内置 Profile（\(profile.id)）规范显式声明绑定，定向配置使用 \(profile.runner)",
                true
            )
        }
    }

    // MARK: - 界面主体

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                headerSection
                if let actionError {
                    ErrorBanner(text: actionError)
                }
                runnerRecognitionCard
                fileSpecificationCard
                if modelType == "llm" {
                    contextSettingsCard
                    if case .registered = target {
                        generationSettingsCard
                    }
                }
                if case .registered = target {
                    identifierManagementCard
                }
                if isLoaded {
                    runtimeMetricsCard
                }
            }
            .padding(Theme.Space.page)
        }
        .frame(minWidth: 580, idealWidth: 620, minHeight: 520)
        .navigationTitle("模型详细设置")
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("关闭") { dismiss() }
                    .keyboardShortcut(.cancelAction)
            }
            ToolbarItemGroup(placement: .primaryAction) {
                actionToolbarButtons
            }
        }
        .onAppear {
            setupInitialDrafts()
        }
    }

    // MARK: - 顶部概要

    private var headerSection: some View {
        HStack(alignment: .top, spacing: 12) {
            ModelTypeIcon(type: modelType)
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 8) {
                    Text(titleText)
                        .font(.title2.weight(.semibold))
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Chip(text: modelTypeLabel)
                    if isWorker {
                        Chip(text: "独立进程", color: Theme.accent)
                    }
                }
                HStack(spacing: 8) {
                    if isLoaded {
                        HStack(spacing: 4) {
                            StatusDot(color: Theme.success, size: 7, glow: true)
                            Text(isWorker ? "独立进程运行中" : "已加载至内存")
                                .font(.caption.weight(.medium))
                                .foregroundStyle(Theme.success)
                        }
                    } else {
                        HStack(spacing: 4) {
                            StatusDot(color: Color.secondary.opacity(0.6), size: 6)
                            Text("未加载")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                    Text("·")
                        .foregroundStyle(.tertiary)
                    Text("模型 ID: \(modelID)")
                        .font(.caption.monospaced())
                        .foregroundStyle(.secondary)
                }
            }
            Spacer(minLength: 0)
        }
    }

    // MARK: - Runner 与识别依据卡片（核心）

    private var runnerRecognitionCard: some View {
        SectionCard(title: "Runner 与识别机制", icon: "cpu") {
            VStack(alignment: .leading, spacing: 10) {
                // 识别的 Runner
                HStack(alignment: .center, spacing: 8) {
                    Text("承载 Runner:")
                        .font(.callout.weight(.medium))
                        .foregroundStyle(.secondary)
                    if let runnerID {
                        Text(runnerID)
                            .font(.callout.weight(.semibold).monospaced())
                            .foregroundStyle(.primary)
                            .textSelection(.enabled)
                        if runnerIsAvailable {
                            Chip(text: "可用就绪", color: Theme.success)
                        } else {
                            Chip(text: "环境未就绪", color: Theme.warning)
                        }
                    } else {
                        Text("未识别 / 待定")
                            .font(.callout)
                            .foregroundStyle(Theme.warning)
                    }
                }

                // Runner 状态与跳转提示
                if let runnerEntry, !runnerEntry.status.available {
                    HStack(spacing: 8) {
                        Label(
                            runnerEntry.status.reason ?? runnerEntry.status.installHint ?? "Runner 引擎环境尚未安装",
                            systemImage: "exclamationmark.triangle.fill"
                        )
                        .font(.caption)
                        .foregroundStyle(Theme.warning)

                        Spacer()

                        Button("去安装引擎") {
                            dismiss()
                            router.goToManagement()
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                    }
                    .padding(8)
                    .background(
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .fill(Theme.warning.opacity(0.1))
                    )
                }

                HairlineDivider()

                // Runner 识别方式
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 6) {
                        Image(systemName: runnerRecognitionInfo.isSuccess ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(runnerRecognitionInfo.isSuccess ? Theme.success : Theme.warning)
                        Text("识别方式: \(runnerRecognitionInfo.method)")
                            .font(.callout.weight(.medium))
                    }
                    Text(runnerRecognitionInfo.detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .padding(.leading, 20)
                }

                if case .repo(let model) = target, model.isDirectory, let inspection {
                    if !inspection.matches.isEmpty {
                        HairlineDivider()
                        VStack(alignment: .leading, spacing: 4) {
                            Text("本地探测签名匹配详情:")
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(.secondary)
                            ForEach(inspection.matches, id: \.detectorID) { match in
                                HStack(alignment: .top, spacing: 6) {
                                    Text("•")
                                        .foregroundStyle(.tertiary)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text("\(match.runner) (detector: \(match.detectorID))")
                                            .font(.caption.weight(.medium).monospaced())
                                        Text(match.reason)
                                            .font(.caption2)
                                            .foregroundStyle(.secondary)
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // MARK: - 文件与规格卡片

    private var fileSpecificationCard: some View {
        SectionCard(title: "模型规格与文件", icon: "folder") {
            VStack(alignment: .leading, spacing: 9) {
                if let path = filePath {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("存储路径")
                            .font(.caption.weight(.medium))
                            .foregroundStyle(.secondary)
                        HStack(spacing: 6) {
                            Text(path)
                                .font(.caption.monospaced())
                                .foregroundStyle(.primary)
                                .lineLimit(2)
                                .textSelection(.enabled)
                            Spacer(minLength: 4)
                            Button {
                                copyPathToClipboard(path)
                            } label: {
                                Label(copiedPath ? "已复制" : "复制路径", systemImage: copiedPath ? "checkmark" : "doc.on.doc")
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                            .help("复制完整路径至剪贴板")

                            Button {
                                revealInFinder(path)
                            } label: {
                                Label("在访达中显示", systemImage: "arrow.up.forward.square")
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                            .help("在 macOS 访达中定位模型文件")
                        }
                    }
                }

                // 文件大小 / 目录形式
                HStack(spacing: 16) {
                    if case .repo(let model) = target {
                        LabeledContent("形态", value: model.isDirectory ? "目录格式" : "单文件格式")
                        LabeledContent("大小", value: model.isDirectory ? "目录" : Format.bytes(model.sizeBytes))
                    } else if case .profile(let profile) = target {
                        if let est = profile.memoryEstimateBytes {
                            LabeledContent("预计内存", value: Format.bytes(est))
                        }
                        LabeledContent("来源仓库", value: profile.sourceRepo)
                    }
                }
                .font(.callout)

                // GGUF 专属元数据
                if case .repo(let model) = target, let meta = model.ggufMetadata {
                    HairlineDivider()
                    VStack(alignment: .leading, spacing: 6) {
                        Text("GGUF 元数据解析")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 6) {
                            LabeledContent("模型架构", value: meta.architecture)
                            if let q = meta.quantizationName {
                                LabeledContent("量化级别", value: q)
                            }
                            if let ctx = meta.contextLength {
                                LabeledContent("原生上下文上限", value: GGUFMetadata.formatTokenCount(ctx))
                            }
                            LabeledContent("张量数量", value: "\(meta.tensorCount)")
                        }
                        .font(.caption)
                    }
                }
            }
        }
    }

    // MARK: - 上下文长度设置卡片（LLM 专属）

    private var contextSettingsCard: some View {
        SectionCard(title: "上下文长度设置", icon: "arrow.left.and.right") {
            VStack(alignment: .leading, spacing: 10) {
                Text("配置该模型运行时的上下文窗口（Context Length）。设置将保存并在重新加载时生效。")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                HStack(spacing: 8) {
                    Text("上下文长度:")
                        .font(.callout.weight(.medium))

                    HStack(spacing: 2) {
                        TextField("4", text: $contextDraft)
                            .textFieldStyle(.plain)
                            .font(.body.monospacedDigit())
                            .frame(width: 50)
                            .multilineTextAlignment(.trailing)
                            .onChange(of: contextDraft) { _, newValue in
                                let filtered = newValue.filter { $0.isNumber || $0 == "." }
                                if filtered != newValue { contextDraft = filtered }
                            }
                        Text("K")
                            .font(.body.weight(.medium).monospacedDigit())
                            .foregroundStyle(.secondary)
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 5)
                    .background(RoundedRectangle(cornerRadius: 6).fill(Theme.inset))
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Theme.hairlineStrong))

                    Spacer(minLength: 8)

                    // 预设快捷按钮
                    HStack(spacing: 4) {
                        ForEach(["2", "4", "8", "16", "32", "64", "128"], id: \.self) { preset in
                            Button("\(preset)K") {
                                contextDraft = preset
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.mini)
                            .tint(contextDraft == preset ? Theme.accent : nil)
                        }
                    }
                }

                HStack {
                    if let contextFeedback {
                        Text(contextFeedback)
                            .font(.caption)
                            .foregroundStyle(Theme.success)
                    }
                    Spacer()
                    Button(isApplyingContext ? "应用中…" : "应用并重载") {
                        applyContextLength()
                    }
                    .buttonStyle(ProminentButtonStyle())
                    .controlSize(.small)
                    .disabled(isApplyingContext || parsedContextTokens == nil || controller.phase != .online)
                }
            }
        }
    }

    private var generationSettingsCard: some View {
        SectionCard(title: "生成参数", icon: "slider.horizontal.3") {
            VStack(alignment: .leading, spacing: 10) {
                Text("请求未显式传入参数时，daemon 使用这里保存的模型默认值。")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                LabeledContent("Temperature") {
                    TextField("Temperature", value: $temperatureDraft, format: .number)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 90)
                }
                LabeledContent("Top P") {
                    TextField("Top P", value: $topPDraft, format: .number)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 90)
                }
                LabeledContent("最大生成 Token") {
                    TextField("最大生成 Token", value: $maxTokensDraft, format: .number)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 90)
                }

                HStack {
                    if let generationFeedback {
                        Text(generationFeedback)
                            .font(.caption)
                            .foregroundStyle(Theme.success)
                    }
                    Spacer()
                    Button(isApplyingGeneration ? "保存中…" : "保存生成参数", action: applyGenerationSettings)
                        .buttonStyle(ProminentButtonStyle())
                        .controlSize(.small)
                        .disabled(isApplyingGeneration || !generationSettingsAreValid || controller.phase != .online)
                }
            }
        }
    }

    // MARK: - 模型 ID 管理卡片（已注册模型专属）

    private var identifierManagementCard: some View {
        SectionCard(title: "模型 ID 管理", icon: "character.cursor.ibeam") {
            VStack(alignment: .leading, spacing: 10) {
                Text("修改该模型在守护进程与客户端 API 中调用的注册 ID。修改后已加载模型会先停止并以新 ID 更新。")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                if let renameError {
                    ErrorBanner(text: renameError)
                }

                HStack(spacing: 8) {
                    TextField("模型 ID", text: $idDraft)
                        .textFieldStyle(.roundedBorder)
                        .font(.body.monospaced())

                    Button(isRenaming ? "保存中…" : "更改 ID") {
                        applyRename()
                    }
                    .buttonStyle(ProminentButtonStyle())
                    .controlSize(.small)
                    .disabled(isRenaming || !isRenameValid || controller.phase != .online)

                    if case .registered(let entry) = target,
                       let orig = entry.originalID,
                       orig != entry.id {
                        Button("恢复原始 ID") {
                            idDraft = orig
                            applyRename()
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(isRenaming || controller.phase != .online)
                    }
                }
            }
        }
    }

    // MARK: - 实时运行监控卡片（已加载专属）

    private var runtimeMetricsCard: some View {
        SectionCard(title: "运行时动态状态", icon: "gauge") {
            VStack(alignment: .leading, spacing: 8) {
                if let loaded = loadedModel {
                    HStack(spacing: 16) {
                        LabeledContent("加速设备", value: loaded.effectiveDevice?.uppercased() ?? "未探测")
                        if let mem = loaded.memoryUsageBytes {
                            LabeledContent("实时内存", value: Format.bytes(mem))
                        }
                        if let est = loaded.memoryEstimate {
                            LabeledContent("预估内存", value: Format.bytes(est))
                        }
                        if let ka = loaded.keepAlive {
                            LabeledContent("驻留时间策略", value: ka)
                        }
                    }
                    .font(.callout)
                }
            }
        }
    }

    // MARK: - 工具栏操作按钮

    @ViewBuilder
    private var actionToolbarButtons: some View {
        if isPerformingAction {
            ProgressView().controlSize(.small)
        } else {
            switch target {
            case .registered(let entry):
                if isLoaded {
                    Button {
                        performUnload(entry.id)
                    } label: {
                        Label("停止运行", systemImage: "stop.circle")
                    }
                    .tint(Theme.danger)
                } else {
                    Button {
                        performLoad(entry.id)
                    } label: {
                        Label("启动加载", systemImage: "play.circle")
                    }
                    .tint(Theme.success)
                }

                Button(role: .destructive) {
                    performUnregister(entry.id)
                } label: {
                    Label("注销模型", systemImage: "trash")
                }

            case .repo(let model):
                if modelType == "tts" && isLoaded {
                    Button {
                        performTTSPreview(model.modelID)
                    } label: {
                        Label(isPreviewingTTS ? "播放中…" : "试听", systemImage: "speaker.wave.2.fill")
                    }
                    .disabled(isPreviewingTTS)
                }

                Button {
                    performRepoRegister(model)
                } label: {
                    Label("注册并加载", systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(model.detectionError != nil || controller.phase != .online)

            case .profile(let profile):
                if !profile.isDownloaded {
                    Button {
                        performProfileInstall(profile)
                    } label: {
                        Label("下载并启动", systemImage: "arrow.down.circle")
                    }
                    .disabled(controller.phase != .online)
                }
            }
        }
    }

    // MARK: - 交互操作逻辑

    private func setupInitialDrafts() {
        let currentTokens = ModelRepository.contextLength(for: modelID)
        let kilo = Double(currentTokens) / 1024.0
        contextDraft = kilo.rounded() == kilo ? String(Int(kilo)) : String(format: "%.2f", kilo)
        idDraft = modelID
        if case .registered(let entry) = target {
            temperatureDraft = entry.temperature ?? 1.0
            topPDraft = entry.topP ?? 0.95
            maxTokensDraft = entry.maxTokens ?? 1024
        }
    }

    private var generationSettingsAreValid: Bool {
        temperatureDraft.isFinite && (0...2).contains(temperatureDraft)
            && topPDraft.isFinite && (0...1).contains(topPDraft)
            && (1...1_048_576).contains(maxTokensDraft)
    }

    private func applyGenerationSettings() {
        guard generationSettingsAreValid else { return }
        isApplyingGeneration = true
        generationFeedback = nil
        Task {
            defer { isApplyingGeneration = false }
            do {
                try await controller.setGenerationSettings(
                    modelID,
                    temperature: temperatureDraft,
                    topP: topPDraft,
                    maxTokens: maxTokensDraft
                )
                generationFeedback = "生成参数已保存"
                await onUpdate()
            } catch {
                actionError = "保存生成参数失败：\(DaemonController.message(for: error))"
            }
        }
    }

    private var parsedContextTokens: Int? {
        let trimmed = contextDraft.trimmingCharacters(in: .whitespaces)
        guard let kilo = Double(trimmed), kilo > 0, kilo <= 1024 else { return nil }
        return Int(kilo * 1024.0)
    }

    private var isRenameValid: Bool {
        let trimmed = idDraft.trimmingCharacters(in: .whitespaces)
        return !trimmed.isEmpty
            && trimmed != modelID
            && trimmed.allSatisfy { $0.isLetter || $0.isNumber || $0 == "." || $0 == "-" || $0 == "_" }
    }

    private func copyPathToClipboard(_ path: String) {
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.setString(path, forType: .string)
        copiedPath = true
        Task {
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            copiedPath = false
        }
    }

    private func revealInFinder(_ path: String) {
        let url = URL(fileURLWithPath: path)
        NSWorkspace.shared.activateFileViewerSelecting([url])
    }

    private var targetIsRepoDirectory: Bool {
        if case .repo(let model) = target {
            return model.isDirectory
        }
        return false
    }

    private func applyContextLength() {
        guard let tokens = parsedContextTokens else { return }
        isApplyingContext = true
        contextFeedback = nil
        ModelRepository.setContextLength(tokens, for: modelID)

        Task {
            defer { isApplyingContext = false }
            if let path = filePath {
                do {
                    let isReg = controller.registeredModels.contains { $0.id == modelID }
                    let token = (!isReg && targetIsRepoDirectory) ? inspection?.routingToken : nil
                    let prov = token == nil ? runnerID : nil
                    try await controller.registerAndLoad(
                        path: path,
                        id: modelID,
                        contextLength: tokens,
                        keepAlive: nil,
                        modelType: token != nil ? nil : (modelType == "llm" ? nil : modelType),
                        provider: prov,
                        routingToken: token
                    )
                    contextFeedback = "上下文设置已应用并生效（\(contextDraft)K）"
                    await onUpdate()
                } catch {
                    actionError = "重载模型失败：\(DaemonController.message(for: error))"
                }
            } else {
                contextFeedback = "上下文设置已保存至本地配置"
            }
        }
    }

    private func applyRename() {
        guard isRenameValid else { return }
        let newID = idDraft.trimmingCharacters(in: .whitespaces)
        isRenaming = true
        renameError = nil

        Task {
            defer { isRenaming = false }
            await controller.rename(modelID, to: newID)
            if controller.lastError != nil {
                renameError = controller.lastError
            } else {
                await onUpdate()
                dismiss()
            }
        }
    }

    private func performLoad(_ id: String) {
        isPerformingAction = true
        Task {
            defer { isPerformingAction = false }
            await controller.loadRegistered(id)
            await onUpdate()
        }
    }

    private func performUnload(_ id: String) {
        isPerformingAction = true
        Task {
            defer { isPerformingAction = false }
            await controller.unload(id)
            await onUpdate()
        }
    }

    private func performUnregister(_ id: String) {
        isPerformingAction = true
        Task {
            defer { isPerformingAction = false }
            await controller.unregister(id)
            await onUpdate()
            dismiss()
        }
    }

    private func performRepoRegister(_ model: RepoModel) {
        isPerformingAction = true
        Task {
            defer { isPerformingAction = false }
            do {
                try await controller.registerAndLoad(
                    path: model.path,
                    id: model.modelID,
                    contextLength: ModelRepository.contextLength(for: model.modelID),
                    keepAlive: nil,
                    modelType: model.isDirectory ? nil : (model.modelType == "llm" ? nil : model.modelType),
                    provider: nil,
                    routingToken: inspection?.routingToken
                )
                await onUpdate()
            } catch {
                actionError = "注册加载失败：\(DaemonController.message(for: error))"
            }
        }
    }

    private func performProfileInstall(_ profile: ModelProfile) {
        isPerformingAction = true
        Task {
            defer { isPerformingAction = false }
            if await controller.installRecommended(profile) {
                await onUpdate()
            }
        }
    }

    private func performTTSPreview(_ id: String) {
        isPreviewingTTS = true
        Task {
            defer { isPreviewingTTS = false }
            do {
                let audio = try await controller.api.synthesizeSpeech(
                    modelID: id,
                    text: "你好，我是 Mac AI 的语音合成，很高兴为你朗读这段文字。",
                    voice: "zf_001"
                )
                let url = FileManager.default.temporaryDirectory
                    .appendingPathComponent("macai-preview-\(UUID().uuidString).wav")
                try audio.write(to: url)
                defer { try? FileManager.default.removeItem(at: url) }
                let process = Process()
                process.executableURL = URL(fileURLWithPath: "/usr/bin/afplay")
                process.arguments = [url.path]
                try process.run()
                process.waitUntilExit()
            } catch {
                actionError = "试听失败：\(DaemonController.message(for: error))"
            }
        }
    }
}
