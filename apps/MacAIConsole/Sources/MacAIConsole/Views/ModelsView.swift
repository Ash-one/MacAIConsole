import SwiftUI

struct ModelsView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(PythonEnvironmentManager.self) private var pythonEnvironments
    @State private var repoModels: [RepoModel] = []
    @State private var showingAddSheet = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.sectionSpacing) {
                if controller.phase != .online {
                    OfflineHint { controller.startDaemon() }
                }
                if let error = controller.lastError {
                    ErrorBanner(text: error)
                }
                if !pendingRecommendations.isEmpty {
                    recommendedSection
                }
                registeredSection
                repoSection
            }
            .padding(Theme.Space.page)
        }
        .navigationTitle("模型管理")
        .toolbar {
            ToolbarItemGroup {
                Button {
                    showingAddSheet = true
                } label: {
                    Label("添加模型", systemImage: "plus")
                }
                .disabled(controller.phase != .online)

                Button {
                    Task {
                        await rescanRepo()
                        try? await controller.refresh()
                    }
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
            }
        }
        .sheet(isPresented: $showingAddSheet, onDismiss: { Task { await rescanRepo() } }) {
            AddModelSheet()
        }
        .task {
            controller.bootstrapIfNeeded()
            await rescanRepo()
        }
    }

    /// 每次都重新扫描仓库目录：放入文件的即刻可见。扫描在后台执行，结果回主线程赋值。
    private func rescanRepo() async {
        repoModels = await ModelRepository.scanInBackground()
    }

    private var loadedIDs: Set<String> {
        Set((controller.info?.loadedModels ?? []).map(\.id))
    }

    /// 已注册的模型 ID（daemon 注册表里有记录就算，不论是否加载）。
    private var registeredIDs: Set<String> {
        Set(controller.registeredModels.map(\.id))
    }

    /// 已注册的模型来源路径集合。仓库灰色状态按 path 匹配——改名不影响它。
    private var registeredPaths: Set<String> {
        Set(controller.registeredModels.compactMap(\.path))
    }

    private var pendingRecommendations: [RecommendedModel] {
        RecommendedModel.builtIns.filter { !$0.isDownloaded }
    }

    /// 按类型分组展示顺序：llm → stt → tts，其余未知类型按字母序殿后。
    private let typeOrder = ["llm", "stt", "tts"]

    private var groupedTypes: [String] {
        let present = Set(controller.registeredModels.map(\.modelType))
        var ordered = typeOrder.filter { present.contains($0) }
        ordered.append(contentsOf: present.subtracting(typeOrder).sorted())
        return ordered
    }

    private func entries(ofType type: String) -> [ModelEntry] {
        controller.registeredModels.filter { $0.modelType == type }
    }

    private func typeLabel(_ type: String) -> String {
        switch type {
        case "llm": "对话模型 · LLM"
        case "stt": "语音识别 · STT"
        case "tts": "语音合成 · TTS"
        default: "其他 · \(type)"
        }
    }

    /// 该模型的 Provider 是否为独立进程（worker 隔离）：加载即拉起子进程，卸载即结束进程。
    private func isWorker(_ entry: ModelEntry) -> Bool {
        let provider = entry.ownedBy.split(separator: "/").last.map(String.init) ?? entry.ownedBy
        return controller.providers.first { $0.descriptor.id == provider }?
            .descriptor.isolation == "worker"
    }

    private var recommendedSection: some View {
        SectionCard(
            title: "推荐模型",
            icon: "sparkles",
            subtitle: "已选择适配 MacAI Provider 的版本，可直接下载到模型仓库"
        ) {
            VStack(spacing: 0) {
                ForEach(pendingRecommendations) { model in
                    let diagnostics = providerDiagnostics(for: model)
                    RecommendedModelRow(
                        model: model,
                        isRegistered: registeredIDs.contains(model.id),
                        isLoaded: loadedIDs.contains(model.id),
                        providerAvailable: controller.providerIsAvailable(model.provider),
                        providerMissing: diagnostics.missing,
                        unavailableReason: diagnostics.reason,
                        pythonSpec: diagnostics.spec
                    ) {
                        Task {
                            if await controller.installRecommended(model) {
                                await rescanRepo()
                            }
                        }
                    }
                    if model.id != pendingRecommendations.last?.id {
                        HairlineDivider()
                    }
                }
            }
        }
    }

    /// 推荐模型行的 Provider 诊断：区分「未装配」（如 Qwen3-ASR 开关未开）与
    /// 「环境未就绪」（缺 Python venv），并给出对应的 Python 环境规格。
    private func providerDiagnostics(for model: RecommendedModel)
        -> (missing: Bool, reason: String?, spec: PythonEnvironmentSpec?)
    {
        let spec = PythonEnvironmentSpec.spec(forProvider: model.provider)
        guard let entry = controller.providers.first(where: { $0.descriptor.id == model.provider }) else {
            return (true, "Provider 未装配：在「设置」中开启对应开关并重启 aiworkd", spec)
        }
        if !entry.status.available {
            return (false, entry.status.reason ?? entry.status.installHint, spec)
        }
        return (false, nil, spec)
    }

    private var registeredSection: some View {
        SectionCard(title: "模型注册 ID", icon: "number.square", infoText: "调用时使用的模型 ID") {
            if controller.registeredModels.isEmpty {
                EmptyHint(
                    text: controller.phase == .online ? "注册表为空" : "守护进程离线，暂无数据",
                    systemImage: "number.square"
                )
            } else {
                VStack(spacing: 10) {
                    ForEach(groupedTypes, id: \.self) { type in
                        ModelTypeSection(type: type, title: typeLabel(type), count: entries(ofType: type).count) {
                            ForEach(entries(ofType: type)) { entry in
                                RegisteredModelRow(
                                    entry: entry,
                                    isLoaded: loadedIDs.contains(entry.id),
                                    isWorkerProcess: isWorker(entry)
                                )
                                if entry.id != entries(ofType: type).last?.id { HairlineDivider() }
                            }
                        }
                    }
                }
            }
        }
    }

    private var repoSection: some View {
        SectionCard(
            title: "模型仓库",
            icon: "archivebox",
            infoText: "自动识别到路径中的模型",
            accessory: {
                Button {
                    openModelRepository()
                } label: {
                    Label("打开路径", systemImage: "folder")
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .help("在访达中打开模型保存路径")
            }
        ) {
            VStack(alignment: .leading, spacing: 10) {
                if repoModels.isEmpty {
                    VStack(spacing: 8) {
                        Text("仓库为空——把 .gguf（llm）或 .bin（stt）文件放进对应文件夹，或点「添加模型」")
                            .font(.callout)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                        Button("添加模型…") { showingAddSheet = true }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                            .disabled(controller.phase != .online)
                    }
                    .frame(maxWidth: .infinity, minHeight: 44)
                } else {
                    VStack(spacing: 10) {
                        ForEach(ModelRepository.folderNames, id: \.self) { type in
                            let entries = repoModels.filter { $0.modelType == type }
                            if !entries.isEmpty {
                                let allRegistered = entries.allSatisfy { registeredPaths.contains($0.path) }
                                ModelTypeSection(type: type, title: typeLabel(type), count: entries.count, dimmed: allRegistered) {
                                    ForEach(entries) { model in
                                        RepoModelRow(
                                            model: model,
                                            // 仓库行只关心「是否已注册」，不同步展示运行时加载状态。
                                            isLoaded: false,
                                            isRegistered: registeredPaths.contains(model.path),
                                            isRecommended: RecommendedModel.builtIns.contains {
                                                $0.matches(repositoryModel: model)
                                            }
                                        )
                                        if model.id != entries.last?.id { HairlineDivider() }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    private func openModelRepository() {
        // 这里只需要目录存在（NSWorkspace.open 对缺失路径会失败），用轻量的 ensureFolders，
        // 不必为建目录做一次全量扫描。
        ModelRepository.ensureFolders()
        NSWorkspace.shared.open(ModelRepository.baseURL)
    }
}


/// 可折叠的类型分组：LLM / STT / TTS。默认展开，chevron 随状态旋转。
/// `dimmed` 为 true 时（仓库区该类型已全部注册）标题降灰，提示整组不重要。
struct ModelTypeSection<Content: View>: View {
    let type: String
    let title: String
    let count: Int
    var dimmed: Bool = false
    @ViewBuilder let content: () -> Content

    @State private var isExpanded = true

    var body: some View {
        DisclosureGroup(isExpanded: $isExpanded) {
            content()
                .padding(.leading, 24)
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "chevron.right")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(dimmed ? .quaternary : .tertiary)
                    .rotationEffect(.degrees(isExpanded ? 90 : 0))
                ModelTypeIcon(type: type)
                    .opacity(dimmed ? 0.55 : 1)
                Text(title)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(dimmed ? Color(nsColor: .secondaryLabelColor) : .primary)
                Spacer(minLength: 8)
                Chip(text: "\(count) 个")
                    .opacity(dimmed ? 0.55 : 1)
            }
            // 折叠头与子条目行同高：与行内 padding 对齐，保证收起时占位一致。
            .padding(.vertical, 10)
            .padding(.horizontal, 6)
            .contentShape(Rectangle())
        }
        .disclosureGroupStyle(PlainDisclosureStyle())
    }
}

/// 无默认指示器的折叠样式：标题行整体可点，展开动画用系统 spring。
struct PlainDisclosureStyle: DisclosureGroupStyle {
    func makeBody(configuration: Configuration) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            configuration.label
                .onTapGesture {
                    withAnimation(.snappy) { configuration.isExpanded.toggle() }
                }
            if configuration.isExpanded {
                configuration.content
            }
        }
    }
}

struct RegisteredModelRow: View {
    @Environment(DaemonController.self) private var controller
    let entry: ModelEntry
    let isLoaded: Bool
    /// true 表示 Provider 为 worker 隔离：加载=拉起独立子进程，卸载=结束该进程。
    let isWorkerProcess: Bool

    @State private var showRenameSheet = false
    @State private var renameDraft = ""

    /// 原始 ID = 注册来源文件名（去扩展名），来自 daemon 记录的 path，改名不影响。
    private var originalID: String? { entry.originalID }
    /// 当前 ID 与原始 ID 不同 = 用户改过名。
    private var isRenamed: Bool {
        guard let originalID else { return false }
        return entry.id != originalID
    }

    var body: some View {
        HStack(spacing: 10) {
            ModelTypeIcon(type: entry.modelType)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 8) {
                    Text(entry.id)
                        .font(.body.weight(.semibold))
                    if isWorkerProcess {
                        Chip(text: "独立进程", color: Theme.accent)
                    }
                }
                Text(entry.ownedBy)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            Spacer(minLength: 12)
            if isLoaded {
                HStack(spacing: 5) {
                    StatusDot(color: Theme.success, size: 6, glow: true)
                    Text(isWorkerProcess ? "运行中" : "已加载")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(Theme.success)
                }
                .help(isWorkerProcess ? "独立进程正在运行" : "模型已加载到内存")
                if controller.busyModelIDs.contains(entry.id) {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    GhostActionButton(
                        systemImage: isWorkerProcess ? "stop.circle" : "eject.circle",
                        help: isWorkerProcess ? "停止独立进程" : "卸载",
                        activeTint: Theme.danger,
                        isDisabled: controller.phase != .online
                    ) {
                        Task { await controller.unload(entry.id) }
                    }
                }
            } else {
                if controller.busyModelIDs.contains(entry.id) {
                    ProgressView()
                        .controlSize(.small)
                } else {
                    GhostActionButton(
                        systemImage: "play.circle",
                        help: isWorkerProcess ? "启动独立进程" : "加载",
                        activeTint: Theme.success,
                        isDisabled: controller.phase != .online
                    ) {
                        Task { await controller.loadRegistered(entry.id) }
                    }
                }
            }
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 8)
        .hoverableRow()
        .contextMenu {
            Button {
                renameDraft = entry.id
                showRenameSheet = true
            } label: {
                Label("更改 ID…", systemImage: "character.cursor.ibeam")
            }
            .disabled(originalID == nil)

            Button {
                if let originalID {
                    Task { await controller.rename(entry.id, to: originalID) }
                }
            } label: {
                Label(isRenamed ? "恢复原始 ID（\(originalID ?? "")）" : "恢复原始 ID",
                      systemImage: "arrow.uturn.backward")
            }
            .disabled(!isRenamed)

            Divider()

            Button(role: .destructive) {
                Task { await controller.unregister(entry.id) }
            } label: {
                Label("删除", systemImage: "trash")
                    .foregroundStyle(.red)
            }
        }
        .sheet(isPresented: $showRenameSheet) {
            RenameModelSheet(currentID: entry.id) { newID in
                Task { await controller.rename(entry.id, to: newID) }
            }
        }
    }
}

/// 更改模型 ID 的输入弹窗。
struct RenameModelSheet: View {
    @Environment(\.dismiss) private var dismiss
    let currentID: String
    let onSubmit: (String) -> Void

    @State private var draft = ""

    private var isValid: Bool {
        let trimmed = draft.trimmingCharacters(in: .whitespaces)
        return !trimmed.isEmpty
            && trimmed != currentID
            && trimmed.allSatisfy { $0.isLetter || $0.isNumber || $0 == "." || $0 == "-" || $0 == "_" }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("更改模型 ID")
                .font(.headline)
            TextField("新的 ID", text: $draft)
                .textFieldStyle(.roundedBorder)
                .onSubmit { if isValid { submit() } }
            Text("只允许字母、数字、点、连字符和下划线。已加载的模型会先停止。")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack {
                Spacer()
                Button("取消", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("确定") { submit() }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(ProminentButtonStyle())
                    .disabled(!isValid)
            }
        }
        .padding(20)
        .frame(width: 380)
        .onAppear { draft = currentID }
    }

    private func submit() {
        dismiss()
        onSubmit(draft.trimmingCharacters(in: .whitespaces))
    }
}

/// 模型仓库中的一行：显示文件与大小，一键注册加载。
/// 已注册（注册表里有该 ID）的行整行变灰，按钮切换为「重新注册」。
/// 注册结果持久化在 daemon 的 SQLite 注册表，daemon 重启后自动恢复清单。
/// llm 行可编辑上下文长度；提交后自动以新上下文重载（daemon 会替换驻留进程）。
struct RepoModelRow: View {
    @Environment(DaemonController.self) private var controller
    let model: RepoModel
    let isLoaded: Bool
    let isRegistered: Bool
    let isRecommended: Bool

    @State private var contextDraft = ""
    @State private var isReloading = false
    @State private var isPreviewing = false

    private var isLLM: Bool { model.modelType == "llm" }
    private var isTTS: Bool { model.modelType == "tts" }
    /// daemon busyModelIDs 只覆盖注册表里已有的 id；仓库行自己再补一个重载中状态。
    private var busy: Bool {
        controller.busyModelIDs.contains(model.modelID) || isReloading
    }

    var body: some View {
        HStack(spacing: 10) {
            ModelTypeIcon(type: model.modelType)
                .opacity(isRegistered ? 0.55 : 1)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(model.fileName)
                        .font(.body.weight(.semibold))
                        .foregroundStyle(isRegistered ? .secondary : .primary)
                    if isRecommended {
                        Image(systemName: "star.fill")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.yellow)
                            .help("推荐模型")
                    }
                }
                Text("\(Format.bytes(model.sizeBytes)) · Models/\(model.modelType)/")
                    .font(.caption)
                    .foregroundStyle(isRegistered ? .quaternary : .tertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 12)
            if isLLM {
                contextEditor
            }
            if isTTS && isLoaded {
                previewButton
            }
            loadControls
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 8)
        .hoverableRow()
        .onAppear { contextDraft = Self.displayValue(ModelRepository.contextLength(for: model.modelID)) }
    }

    /// TTS 试听：合成一句固定文本并立即播放。
    private var previewButton: some View {
        Button {
            Task { await preview() }
        } label: {
            if isPreviewing {
                ProgressView().controlSize(.small)
            } else {
                Label("试听", systemImage: "speaker.wave.2.fill")
            }
        }
        .labelStyle(.titleAndIcon)
        .buttonStyle(.bordered)
        .controlSize(.small)
        .disabled(isPreviewing || controller.phase != .online)
        .help("合成一句示例文本并播放")
    }

    private func preview() async {
        isPreviewing = true
        defer { isPreviewing = false }
        do {
            let audio = try await controller.api.synthesizeSpeech(
                modelID: model.modelID,
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
            if process.terminationStatus != 0 {
                controller.lastError = "播放失败（afplay 退出码 \(process.terminationStatus)）"
            }
        } catch {
            controller.lastError = "试听失败：\(DaemonController.message(for: error))"
        }
    }

    /// 存储值 → 显示 K 数值（4096 → "4"，单位由界面固定显示）。
    private static func displayValue(_ tokens: Int) -> String {
        let kilo = Double(tokens) / 1024.0
        if kilo.rounded() == kilo { return String(Int(kilo)) }
        return String(format: "%.2f", kilo)
    }

    /// 输入 K 数值 → token 数（"4" → 4096，"0.5" → 512）。
    private static func parseValue(_ text: String) -> Int? {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard let kilo = Double(trimmed), kilo > 0, kilo <= 1024 else { return nil }
        return Int(kilo * 1024.0)
    }

    /// llm 的上下文长度编辑：改动了才出现「应用」，点击即保存并自动重载。
    private var contextEditor: some View {
        HStack(spacing: 4) {
            Text("上下文")
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack(spacing: 2) {
                TextField("4", text: $contextDraft)
                    .font(.caption.monospacedDigit())
                    .textFieldStyle(.plain)
                    .frame(width: 42)
                    .multilineTextAlignment(.trailing)
                    .onChange(of: contextDraft) { _, value in
                        let filtered = value.filter { $0.isNumber || $0 == "." }
                        if filtered != value { contextDraft = filtered }
                    }
                Text("K")
                    .font(.caption.weight(.medium).monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 6)
            .padding(.vertical, 4)
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(Theme.inset)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(Theme.hairlineStrong)
            )
            .onSubmit(applyContext)
            if contextChanged, parsedContext != nil {
                Button(isReloading ? "重载中…" : "应用") { applyContext() }
                    .buttonStyle(ProminentButtonStyle())
                    .controlSize(.small)
                    .disabled(!isValidContext || isReloading || controller.phase != .online)
            }
        }
    }

    private var contextChanged: Bool {
        guard let value = parsedContext else { return false }
        return value != ModelRepository.contextLength(for: model.modelID)
    }

    private var parsedContext: Int? { Self.parseValue(contextDraft) }

    private var isValidContext: Bool {
        guard let value = parsedContext else { return false }
        return value >= 256 && value <= 1024 * 1024
    }

    private func applyContext() {
        guard contextChanged, isValidContext, !isReloading else { return }
        let value = parsedContext!
        // 先落盘设置（下次加载沿用），再带新上下文重新注册加载——daemon 对已驻留模型
        // 会先卸载旧句柄再启动新进程，等效于热重载。
        ModelRepository.setContextLength(value, for: model.modelID)
        isReloading = true
        Task {
            defer { isReloading = false }
            do {
                try await controller.registerAndLoad(
                    path: model.path,
                    id: model.modelID,
                    contextLength: value,
                    keepAlive: nil,
                    modelType: nil
                )
            } catch {
                controller.lastError = "重载失败：\(DaemonController.message(for: error))"
            }
        }
    }

    @ViewBuilder
    private var loadControls: some View {
        if isLoaded && !contextChanged {
            HStack(spacing: 5) {
                StatusDot(color: Theme.success, size: 6, glow: true)
                Text("已加载")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(Theme.success)
            }
            .help("模型已加载到内存")
        }
        if busy {
            ProgressView().controlSize(.small)
        } else {
            Button(isRegistered ? "重新注册" : "注册并加载") {
                Task { await register() }
            }
            .buttonStyle(.bordered)
            .tint(isRegistered ? Color(nsColor: .secondaryLabelColor) : Theme.accent)
            .font(.callout.weight(isRegistered ? .regular : .medium))
            .help(isRegistered ? "以当前设置重新注册并加载" : "注册到运行时并立即加载")
            .disabled(controller.phase != .online)
        }
    }

    private func register() async {
        do {
            try await controller.registerAndLoad(
                path: model.path,
                id: model.modelID,
                contextLength: ModelRepository.contextLength(for: model.modelID),
                keepAlive: nil,
                modelType: isLLM ? nil : model.modelType,
                provider: model.provider
            )
        } catch {
            controller.lastError = "加载失败：\(DaemonController.message(for: error))"
        }
    }
}

struct RecommendedModelRow: View {
    @Environment(DaemonController.self) private var controller
    @Environment(PythonEnvironmentManager.self) private var pythonEnvironments
    let model: RecommendedModel
    let isRegistered: Bool
    let isLoaded: Bool
    let providerAvailable: Bool
    /// true 表示 daemon 未装配该 Provider（如 Qwen3-ASR 开关未开），需先去设置启用。
    let providerMissing: Bool
    /// Provider 不可用的具体原因，来自 daemon 的 reason / install_hint。
    let unavailableReason: String?
    /// 对应的 Python 环境规格；nil 表示该 Provider 不依赖 Python（如 whisper.cpp）。
    let pythonSpec: PythonEnvironmentSpec?
    let action: () -> Void

    private var isBusy: Bool {
        controller.busyRecommendationIDs.contains(model.id)
    }

    private var buttonTitle: String {
        if !model.isDownloaded {
            return isRegistered ? "补全文件" : (providerAvailable ? "下载并启动" : "下载模型")
        }
        if isRegistered { return "启动" }
        return "注册并启动"
    }

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            ModelTypeIcon(type: model.modelType)
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 7) {
                    Text(model.title)
                        .font(.body.weight(.semibold))
                    Chip(text: model.provider)
                }
                Text(model.summary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                HStack(spacing: 5) {
                    Text(Format.bytes(model.estimatedSizeBytes))
                    Text("·")
                    Link("Hugging Face", destination: model.repositoryURL)
                    if !providerAvailable {
                        Text("· 当前仅下载，无法启动")
                    }
                }
                .font(.caption2)
                .foregroundStyle(.tertiary)
                if !providerAvailable {
                    providerGuidance
                }
            }
            Spacer(minLength: 12)
            if isLoaded {
                HStack(spacing: 5) {
                    StatusDot(color: Theme.success, size: 6, glow: true)
                    Text("运行中")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(Theme.success)
                }
            } else if isBusy {
                HStack(spacing: 7) {
                    ProgressView().controlSize(.small)
                    Text(model.isDownloaded ? "正在启动…" : "正在下载…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } else {
                Button(buttonTitle, action: action)
                    .buttonStyle(ProminentButtonStyle())
                    .controlSize(.small)
                    .disabled(controller.phase != .online || (model.isDownloaded && !providerAvailable))
            }
        }
        .padding(.vertical, 11)
        .padding(.horizontal, 8)
        .hoverableRow()
    }

    /// Provider 不可用时的引导：缺失说明去设置启用；缺 Python 环境就提供就地安装。
    @ViewBuilder
    private var providerGuidance: some View {
        HStack(spacing: 8) {
            Label(
                unavailableReason ?? "Provider 环境未就绪",
                systemImage: "exclamationmark.triangle.fill"
            )
            .font(.caption2)
            .foregroundStyle(Theme.warning)

            if let spec = pythonSpec, !providerMissing {
                envInstallControls(for: spec)
            }
        }
    }

    @ViewBuilder
    private func envInstallControls(for spec: PythonEnvironmentSpec) -> some View {
        let state = pythonEnvironments.state(for: spec)
        switch state.phase {
        case .creatingVenv, .installingPackages:
            ProgressView().controlSize(.small)
            Text(state.stepText)
                .font(.caption2)
                .foregroundStyle(.secondary)
            Button("取消") { pythonEnvironments.cancel(spec) }
                .buttonStyle(.bordered)
                .controlSize(.mini)
        case .failed:
            Button("重试安装环境") { pythonEnvironments.install(spec) }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .help(state.errorMessage ?? "")
        case .idle:
            if !pythonEnvironments.isInstalled(spec) {
                Button("安装运行环境") { pythonEnvironments.install(spec) }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .help("在仓库 .build/ 下创建 Python 3.12 环境并安装固定版本依赖")
            }
        }
    }
}
