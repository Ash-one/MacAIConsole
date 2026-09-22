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
    @State private var copiedPath = false

    // 歧义匹配时的选中项
    @State private var selectedMatch: LocalDetectorMatch? = nil

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
                if let selectedMatch {
                    return selectedMatch.runner
                }
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
                        if let selectedMatch {
                            return (
                                "已选定承载 Runner",
                                "已选定 \(selectedMatch.runner)（探测器: \(selectedMatch.detectorID)），匹配依据：\(selectedMatch.reason)",
                                true
                            )
                        }
                        let runners = inspection.matches.map(\.runner).joined(separator: ", ")
                        return ("多重匹配歧义（Ambiguous）", "匹配到多个候选 Runner（\(runners)），请在下方选择一个 Runner 进行注册加载", false)
                    case "unsupported":
                        let diag = inspection.diagnostics.first ?? "暂未识别到可承载的 Runner：请检查模型是否下载完整，或新建/安装对应 Runner"
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
                if case .registered = target {
                    identifierManagementCard
                }
                if modelType == "tts" {
                    voiceAuditionCard
                }
                if modelType == "llm" {
                    contextSettingsCard
                    if case .registered = target {
                        generationSettingsCard
                    }
                }
                if isLoaded {
                    runtimeMetricsCard
                }
                runnerRecognitionCard
                fileSpecificationCard
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
                        if inspection.matches.count > 1 || inspection.status == "ambiguous" {
                            VStack(alignment: .leading, spacing: 8) {
                                HStack {
                                    Text("候选承载 Runner（匹配到 \(inspection.matches.count) 个）：")
                                        .font(.caption.weight(.semibold))
                                        .foregroundStyle(.secondary)
                                    if selectedMatch == nil {
                                        Text("请选择一个承载引擎")
                                            .font(.caption)
                                            .foregroundStyle(Theme.warning)
                                    }
                                }

                                ForEach(inspection.matches) { match in
                                    let isSelected = selectedMatch?.id == match.id
                                    HStack(alignment: .center, spacing: 10) {
                                        Image(systemName: isSelected ? "checkmark.circle.fill" : "circle")
                                            .font(.body)
                                            .foregroundStyle(isSelected ? Theme.accent : Color.secondary.opacity(0.6))

                                        VStack(alignment: .leading, spacing: 3) {
                                            HStack(spacing: 6) {
                                                Text(match.runner)
                                                    .font(.subheadline.weight(.semibold).monospaced())
                                                Chip(text: match.capability.uppercased())
                                                Text("(\(match.detectorID))")
                                                    .font(.caption2)
                                                    .foregroundStyle(.tertiary)
                                            }
                                            Text(match.reason)
                                                .font(.caption)
                                                .foregroundStyle(.secondary)
                                        }

                                        Spacer()

                                        Button(isSelected ? "已选定" : "选择") {
                                            selectedMatch = match
                                        }
                                        .buttonStyle(.bordered)
                                        .controlSize(.small)
                                        .tint(isSelected ? Theme.accent : nil)
                                    }
                                    .padding(10)
                                    .background(
                                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                                            .fill(isSelected ? Theme.accent.opacity(0.08) : Color.primary.opacity(0.03))
                                    )
                                    .overlay(
                                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                                            .stroke(isSelected ? Theme.accent.opacity(0.4) : Color.primary.opacity(0.08), lineWidth: 1)
                                    )
                                    .contentShape(Rectangle())
                                    .onTapGesture {
                                        selectedMatch = match
                                    }
                                }
                            }
                        } else {
                            VStack(alignment: .leading, spacing: 4) {
                                Text("本地探测签名匹配详情:")
                                    .font(.caption.weight(.semibold))
                                    .foregroundStyle(.secondary)
                                ForEach(inspection.matches) { match in
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
                HStack(spacing: 20) {
                    if case .repo(let model) = target {
                        HStack(spacing: 6) {
                            Text("形态:")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(model.isDirectory ? "目录格式" : "单文件格式")
                                .font(.caption.weight(.medium))
                                .foregroundStyle(.primary)
                        }
                        HStack(spacing: 6) {
                            Text("大小:")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(model.isDirectory ? "目录" : Format.bytes(model.sizeBytes))
                                .font(.caption.weight(.semibold).monospacedDigit())
                                .foregroundStyle(.primary)
                        }
                    } else if case .profile(let profile) = target {
                        if let est = profile.memoryEstimateBytes {
                            HStack(spacing: 6) {
                                Text("预计内存:")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                Text(Format.bytes(est))
                                    .font(.caption.weight(.semibold).monospacedDigit())
                                    .foregroundStyle(.primary)
                            }
                        }
                        HStack(spacing: 6) {
                            Text("来源仓库:")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(profile.sourceRepo)
                                .font(.caption.weight(.medium))
                                .foregroundStyle(.primary)
                        }
                    } else if case .registered(let entry) = target, let path = entry.path {
                        let isDir = (try? FileManager.default.attributesOfItem(atPath: path)[.type] as? FileAttributeType) == .typeDirectory
                        HStack(spacing: 6) {
                            Text("形态:")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(isDir ? "目录格式" : "单文件格式")
                                .font(.caption.weight(.medium))
                                .foregroundStyle(.primary)
                        }
                        if !isDir, let size = try? FileManager.default.attributesOfItem(atPath: path)[.size] as? Int64 {
                            HStack(spacing: 6) {
                                Text("大小:")
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                Text(Format.bytes(UInt64(max(0, size))))
                                    .font(.caption.weight(.semibold).monospacedDigit())
                                    .foregroundStyle(.primary)
                            }
                        }
                    }
                }

                // GGUF 专属元数据
                if case .repo(let model) = target, let meta = model.ggufMetadata {
                    HairlineDivider()
                    VStack(alignment: .leading, spacing: 6) {
                        Text("GGUF 元数据解析")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.secondary)
                        LazyVGrid(columns: [GridItem(.flexible()), GridItem(.flexible())], spacing: 6) {
                            metaItem(label: "模型架构", value: meta.architecture)
                            if let q = meta.quantizationName {
                                metaItem(label: "量化级别", value: q)
                            }
                            if let ctx = meta.contextLength {
                                metaItem(label: "原生上下文上限", value: GGUFMetadata.formatTokenCount(ctx))
                            }
                            metaItem(label: "张量数量", value: "\(meta.tensorCount)")
                        }
                    }
                }
            }
        }
    }

    // MARK: - 音色与试听卡片（TTS 专属）

    private var voiceAuditionCard: some View {
        SectionCard(title: "音色与试听", icon: "speaker.wave.2") {
            VoiceSelectionControl(modelID: modelID, allowsAudition: isLoaded)
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

    private static let standardKeepAliveChoices: [(label: String, value: String)] = [
        ("1 分钟", "1m"),
        ("5 分钟", "5m"),
        ("10 分钟", "10m"),
        ("30 分钟", "30m"),
        ("60 分钟", "60m"),
        ("120 分钟", "120m"),
        ("始终常驻", "always"),
    ]

    private func keepAliveChoices(current: String?) -> [(label: String, value: String)] {
        var choices = Self.standardKeepAliveChoices
        if let current, !current.isEmpty, !choices.contains(where: { $0.value == current }) {
            choices.append((current, current))
        }
        return choices
    }

    // MARK: - 实时运行监控卡片（已加载专属）

    private var runtimeMetricsCard: some View {
        SectionCard(title: "运行时动态状态", icon: "gauge") {
            if let loaded = loadedModel {
                HStack(spacing: 10) {
                    metricTile(title: "加速设备", icon: "bolt.fill") {
                        if let dev = loaded.effectiveDevice, !dev.isEmpty {
                            Chip(
                                text: dev.uppercased(),
                                color: dev.lowercased() == "metal" ? Theme.warning : Theme.info
                            )
                        } else {
                            Text("未探测")
                                .font(.callout.weight(.medium))
                                .foregroundStyle(.secondary)
                        }
                    }

                    metricTile(title: "实时驻留内存", icon: "memorychip") {
                        if let mem = loaded.memoryUsageBytes, mem > 0 {
                            Text(Format.bytes(mem))
                                .font(.callout.weight(.semibold).monospacedDigit())
                                .foregroundStyle(.primary)
                        } else if modelType == "stt" {
                            Text("无常驻进程")
                                .font(.callout)
                                .foregroundStyle(.secondary)
                        } else {
                            Text("不可测")
                                .font(.callout)
                                .foregroundStyle(.secondary)
                        }
                    }

                    if let est = loaded.memoryEstimate {
                        metricTile(title: "预估内存需求", icon: "chart.bar") {
                            Text(Format.bytes(est))
                                .font(.callout.weight(.medium).monospacedDigit())
                                .foregroundStyle(.secondary)
                        }
                    }

                    metricTile(title: "驻留时间策略", icon: "clock") {
                        Picker("驻留时间策略", selection: Binding(
                            get: { loaded.keepAlive?.isEmpty == false ? loaded.keepAlive! : "always" },
                            set: { newValue in
                                Task {
                                    await controller.setKeepAlive(modelID, keepAlive: newValue)
                                    await onUpdate()
                                }
                            }
                        )) {
                            ForEach(keepAliveChoices(current: loaded.keepAlive), id: \.value) { choice in
                                Text(choice.label).tag(choice.value)
                            }
                        }
                        .pickerStyle(.menu)
                        .labelsHidden()
                        .fixedSize()
                    }
                }
            }
        }
    }

    private func metricTile<Content: View>(
        title: String,
        icon: String? = nil,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 4) {
                if let icon {
                    Image(systemName: icon)
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
                Text(title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            HStack {
                content()
                Spacer(minLength: 0)
            }
            .frame(height: 24)
        }
        .padding(.horizontal, 10)
        .padding(.vertical, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .fill(Color.primary.opacity(0.035))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 8, style: .continuous)
                .strokeBorder(Theme.hairline, lineWidth: 1)
        )
    }

    private func metaItem(label: String, value: String) -> some View {
        HStack(spacing: 6) {
            Text("\(label):")
                .font(.caption)
                .foregroundStyle(.secondary)
            Text(value)
                .font(.caption.weight(.medium))
                .foregroundStyle(.primary)
            Spacer(minLength: 0)
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
                Button {
                    performRepoRegister(model)
                } label: {
                    Label("注册并加载", systemImage: "arrow.triangle.2.circlepath")
                }
                .disabled(model.detectionError != nil || controller.phase != .online || !canRouteDirectory)
                .help(repoRegisterHelp)

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

    /// 目录型模型若被唯一识别且可用，或用户在候选列表中已明确选择且该 Runner 可用，即可注册。
    private var canRouteDirectory: Bool {
        guard case .repo(let model) = target else { return true }
        if !model.isDirectory { return true }
        if let selectedMatch {
            let entry = controller.providers.first { $0.descriptor.id == selectedMatch.runner }
            return entry?.status.available ?? false
        }
        return inspection?.status == "recognized" && inspection?.runnerAvailable == true
    }

    private var repoRegisterHelp: String {
        if case .repo(let model) = target, model.isDirectory {
            if let selectedMatch {
                let entry = controller.providers.first { $0.descriptor.id == selectedMatch.runner }
                let avail = entry?.status.available ?? false
                return avail ? "使用所选 Runner（\(selectedMatch.runner)）注册并加载" : "所选 Runner 引擎环境尚未就绪"
            }
            if let inspection, inspection.status != "recognized" {
                return inspection.status == "ambiguous"
                    ? "匹配到多个候选 Runner，请在下方选择一个 Runner 进行注册加载"
                    : "暂未识别到可承载的 Runner：请检查模型是否下载完整，或新建/安装对应 Runner"
            }
        }
        return "注册到运行时并立即加载"
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
                    let token = (!isReg && targetIsRepoDirectory) ? (selectedMatch?.routingToken ?? inspection?.routingToken) : nil
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
                let token = selectedMatch?.routingToken ?? inspection?.routingToken
                let prov = selectedMatch?.runner
                try await controller.registerAndLoad(
                    path: model.path,
                    id: model.modelID,
                    contextLength: ModelRepository.contextLength(for: model.modelID),
                    keepAlive: nil,
                    modelType: model.isDirectory ? nil : (model.modelType == "llm" ? nil : model.modelType),
                    provider: token != nil ? nil : prov,
                    routingToken: token
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
}
