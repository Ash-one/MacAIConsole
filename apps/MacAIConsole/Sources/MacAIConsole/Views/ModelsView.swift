import SwiftUI

struct ModelsView: View {
    @Environment(DaemonController.self) private var controller
    @Environment(AppRouter.self) private var router
    @State private var repoModels: [RepoModel] = []
    @State private var inspections: [String: LocalInspection] = [:]
    @State private var showingAddSheet = false
    @State private var showingRunnerEditor = false
    @State private var selectedModel: ModelDetailTarget?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.sectionSpacing) {
                if controller.phase != .online {
                    OfflineHint { controller.startDaemon() }
                }
                if let error = controller.lastError {
                    ErrorBanner(text: error)
                }
                engineSection
                if !pendingRecommendations.isEmpty {
                    recommendedSection
                }
                registeredSection
                repoSection
            }
            .padding(Theme.Space.page)
        }
        .navigationTitle("管理")
        .toolbar {
            ToolbarItemGroup {
                Button {
                    showingAddSheet = true
                } label: {
                    Label("添加模型", systemImage: "plus")
                }
                .disabled(controller.phase != .online)

                Button {
                    showingRunnerEditor = true
                } label: {
                    Label("新建 Runner", systemImage: "hammer")
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
        .sheet(isPresented: $showingRunnerEditor) {
            ScriptRunnerEditorSheet()
                .environment(controller)
        }
        .sheet(item: $selectedModel) { target in
            ModelDetailSheet(
                target: target,
                inspection: inspection(for: target),
                onUpdate: {
                    await rescanRepo()
                    try? await controller.refresh()
                }
            )
            .environment(controller)
            .environment(router)
        }
        .task {
            controller.bootstrapIfNeeded()
            await rescanRepo()
            try? await controller.refresh()
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

    /// 每次都重新扫描仓库目录：放入文件的即刻可见。扫描在后台执行，结果回主线程赋值。
    private func rescanRepo() async {
        repoModels = await ModelRepository.scanInBackground()
        let directories = repoModels.filter(\.isDirectory).map(\.path)
        inspections = Dictionary(uniqueKeysWithValues: (try? await controller.api.inspectModels(paths: directories))?.map { ($0.path, $0) } ?? [])
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

    private var pendingRecommendations: [ModelProfile] {
        controller.modelProfiles.filter { !$0.isDownloaded }
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

    private func runnerTypeOrder(_ runner: RunnerEntry) -> Int {
        let matchingProvider = controller.providers.first { $0.descriptor.id == runner.id }
        let caps = matchingProvider?.descriptor.capabilities ?? runner.capabilities ?? []
        if caps.contains("chat") || caps.contains("completion") {
            return 0
        } else if caps.contains("speech_to_text") {
            return 1
        } else if caps.contains("text_to_speech") {
            return 2
        }
        return 3
    }

    private var sortedRunners: [RunnerEntry] {
        controller.runners.sorted { a, b in
            let orderA = runnerTypeOrder(a)
            let orderB = runnerTypeOrder(b)
            if orderA != orderB {
                return orderA < orderB
            }
            return a.id < b.id
        }
    }

    private var engineSection: some View {
        SectionCard(
            title: "引擎",
            icon: "square.stack.3d.up",
            subtitle: "由 daemon 管理的推理引擎环境，按需安装后即可运行对应类型的模型"
        ) {
            if sortedRunners.isEmpty {
                EmptyHint(
                    text: controller.phase == .online ? "正在探测 Runner 引擎…" : "守护进程离线，暂无引擎数据",
                    systemImage: "square.stack.3d.up"
                )
            } else {
                VStack(spacing: 0) {
                    ForEach(sortedRunners) { runner in
                        let matchingProvider = controller.providers.first { $0.descriptor.id == runner.id }
                        EngineRow(
                            runner: runner,
                            provider: matchingProvider,
                            busy: controller.busyRunnerIDs.contains(runner.id)
                        ) {
                            Task { await controller.installRunner(runner.id) }
                        }
                        if runner.id != sortedRunners.last?.id {
                            HairlineDivider()
                        }
                    }
                }
            }
        }
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
                    ProfileModelRow(
                        model: model,
                        isRegistered: registeredIDs.contains(model.id),
                        isLoaded: loadedIDs.contains(model.id),
                        providerAvailable: controller.providerIsAvailable(model.runner),
                        providerMissing: diagnostics.missing,
                        unavailableReason: diagnostics.reason,
                        onSelect: { selectedModel = .profile(model) }
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

    /// 推荐模型行的 Provider 诊断：区分「未装配」与「环境未就绪」
    /// （daemon 返回的 reason / install_hint）。
    /// 引擎安装引导统一指向设置页的「引擎」区块（daemon /api/runners）。
    private func providerDiagnostics(for model: ModelProfile)
        -> (missing: Bool, reason: String?)
    {
        guard let entry = controller.providers.first(where: { $0.descriptor.id == model.runner }) else {
            return (true, "Profile 指定的 Runner 当前未装配")
        }
        if !entry.status.available {
            return (false, entry.status.reason ?? entry.status.installHint)
        }
        return (false, nil)
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
                                    isWorkerProcess: isWorker(entry),
                                    onSelect: { selectedModel = .registered(entry) }
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
                                            inspection: inspections[model.path],
                                            // 仓库行只关心「是否已注册」，不同步展示运行时加载状态。
                                            isLoaded: false,
                                            isRegistered: registeredPaths.contains(model.path),
                                            isRecommended: controller.modelProfiles.contains {
                                                $0.localURL.standardizedFileURL == URL(fileURLWithPath: model.path).standardizedFileURL
                                            },
                                            onSelect: { selectedModel = .repo(model) }
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
    let onSelect: () -> Void

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
            HStack(spacing: 8) {
                Text(entry.id)
                    .font(.body.weight(.semibold))
                if isWorkerProcess {
                    Chip(text: "独立进程", color: Theme.accent)
                }
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
        .onTapGesture { onSelect() }
        .help("点击查看详细设置")
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
    let inspection: LocalInspection?
    let isLoaded: Bool
    let isRegistered: Bool
    let isRecommended: Bool
    let onSelect: () -> Void

    @State private var contextDraft = ""
    @State private var isReloading = false
    @State private var isPreviewing = false

    private var isLLM: Bool { model.modelType == "llm" }
    private var isTTS: Bool { model.modelType == "tts" }
    /// daemon busyModelIDs 只覆盖注册表里已有的 id；仓库行自己再补一个重载中状态。
    private var busy: Bool {
        controller.busyModelIDs.contains(model.modelID) || isReloading
    }

    private var canRouteDirectory: Bool {
        !model.isDirectory || (inspection?.status == "recognized" && inspection?.runnerAvailable == true)
    }

    var body: some View {
        HStack(spacing: 10) {
            ModelTypeIcon(type: model.modelType)
                .opacity(isRegistered ? 0.55 : 1)
            HStack(spacing: 8) {
                Text(model.fileName)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(isRegistered ? .secondary : .primary)
                if isRecommended {
                    Image(systemName: "star.fill")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.yellow)
                        .help("推荐模型")
                }
                if model.isDirectory {
                    Chip(text: "目录")
                } else {
                    Text(Format.bytes(model.sizeBytes))
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                }
                if let error = model.detectionError {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .font(.caption)
                        .foregroundStyle(Theme.warning)
                        .help("GGUF 检测失败：\(error)")
                }
            }
            Spacer(minLength: 12)
            if isLLM, !model.isDirectory, model.detectionError == nil {
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
        .onTapGesture { onSelect() }
        .help("点击查看详细设置")
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
                TextField(
                    "上下文长度",
                    text: $contextDraft,
                    prompt: Text("4")
                )
                    .labelsHidden()
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
        .help(contextHelp)
    }

    private var contextHelp: String {
        if let contextLength = model.ggufMetadata?.contextLength {
            return "模型声明的原生上下文上限为 \(GGUFMetadata.formatTokenCount(contextLength))"
        }
        return "设置模型运行上下文长度"
    }

    private var contextChanged: Bool {
        guard let value = parsedContext else { return false }
        return value != ModelRepository.contextLength(for: model.modelID)
    }

    private var parsedContext: Int? { Self.parseValue(contextDraft) }

    private var isValidContext: Bool {
        guard let value = parsedContext else { return false }
        let detectedLimit = model.ggufMetadata?.contextLength ?? 1024 * 1024
        return value >= 256 && value <= min(detectedLimit, 1024 * 1024)
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
            .help(loadHelp)
            .disabled(controller.phase != .online || model.detectionError != nil || !canRouteDirectory)
        }
    }

    private var loadHelp: String {
        if let detectionError = model.detectionError {
            return "GGUF 检测失败：\(detectionError)"
        }
        if model.isDirectory, let inspection, inspection.status != "recognized" {
            return inspection.status == "ambiguous" ? "多个 Runner 匹配；请使用 CLI 显式指定 provider" : (inspection.diagnostics.first ?? "目录未被已安装 Runner 支持")
        }
        if model.isDirectory, inspection?.runnerAvailable == false {
            return "匹配的 Runner 环境尚未就绪；请先在设置 → 引擎中安装"
        }
        return isRegistered ? "以当前设置重新注册并加载" : "注册到运行时并立即加载"
    }

    private func register() async {
        do {
            try await controller.registerAndLoad(
                path: model.path,
                id: model.modelID,
                contextLength: ModelRepository.contextLength(for: model.modelID),
                keepAlive: nil,
                modelType: model.isDirectory ? nil : (isLLM ? nil : model.modelType),
                provider: nil,
                routingToken: inspection?.routingToken
            )
        } catch {
            controller.lastError = "加载失败：\(DaemonController.message(for: error))"
        }
    }
}

/// 单个 Runner 引擎行：展示短名、完整 ID、能力类型（LLM/STT/TTS）、
/// 就绪/未安装/失败状态与行内安装/重试按钮。
struct EngineRow: View {
    @Environment(DaemonController.self) private var controller
    let runner: RunnerEntry
    let provider: ProviderEntry?
    let busy: Bool
    let onInstall: () -> Void

    private var shortName: String {
        let raw = runner.id.components(separatedBy: ".").last ?? runner.id
        if raw.hasSuffix("-cpp") {
            return raw.replacingOccurrences(of: "-cpp", with: ".cpp")
        }
        return raw
    }

    private var typeTag: String? {
        let caps = provider?.descriptor.capabilities ?? runner.capabilities ?? []
        if caps.contains("chat") || caps.contains("completion") {
            return "LLM"
        } else if caps.contains("speech_to_text") {
            return "STT"
        } else if caps.contains("text_to_speech") {
            return "TTS"
        }
        return nil
    }

    private var phaseStatus: (text: String, color: Color, canInstall: Bool) {
        if runner.phase == "ready" {
            var text = "已就绪"
            if let device = provider?.status.effectiveDevice, !device.isEmpty {
                text += " · \(device)"
            }
            return (text, Theme.success, false)
        }
        if runner.phase == "failed" {
            return ("环境失败", Theme.danger, true)
        }
        return ("未安装", Color.secondary, true)
    }

    var body: some View {
        HStack(spacing: 10) {
            ModelTypeIcon(type: (typeTag ?? "other").lowercased())
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(shortName)
                        .font(.body.weight(.medium))
                    if let typeTag {
                        Chip(text: typeTag)
                    }
                }
                Text(runner.id)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 12)
            if busy {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("正在安装…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } else {
                let status = phaseStatus
                HStack(spacing: 5) {
                    StatusDot(color: status.color, size: 6, glow: status.color == Theme.success)
                    Text(status.text)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(status.color)
                }
                if status.canInstall {
                    Button(runner.phase == "failed" ? "重试" : "安装") {
                        onInstall()
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(controller.phase != .online)
                }
            }
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 4)
    }
}

/// 精简后的推荐模型行：清晰展示模型名、短 Runner 标签、单行规格说明与操作按钮。
struct ProfileModelRow: View {
    @Environment(DaemonController.self) private var controller
    let model: ModelProfile
    let isRegistered: Bool
    let isLoaded: Bool
    let providerAvailable: Bool
    let providerMissing: Bool
    let unavailableReason: String?
    let onSelect: () -> Void
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

    private var shortRunnerName: String {
        let raw = model.runner.components(separatedBy: ".").last ?? model.runner
        if raw.hasSuffix("-cpp") {
            return raw.replacingOccurrences(of: "-cpp", with: ".cpp")
        }
        return raw
    }

    var body: some View {
        HStack(alignment: .center, spacing: 10) {
            ModelTypeIcon(type: model.modelType)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(model.name)
                        .font(.body.weight(.semibold))
                    Chip(text: shortRunnerName)
                    if !providerAvailable {
                        HStack(spacing: 3) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .font(.caption2)
                            Text("引擎未就绪")
                                .font(.caption2.weight(.medium))
                        }
                        .foregroundStyle(Theme.warning)
                        .help(unavailableReason ?? "对应的 Runner 引擎尚未安装，请在上方引擎列表中安装")
                    }
                }
                HStack(spacing: 5) {
                    if let estimate = model.memoryEstimateBytes {
                        Text("预计内存 \(Format.bytes(estimate))")
                    }
                    if let repositoryURL = model.repositoryURL {
                        if model.memoryEstimateBytes != nil {
                            Text("·")
                        }
                        Link("Hugging Face", destination: repositoryURL)
                    }
                }
                .font(.caption2)
                .foregroundStyle(.tertiary)
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
        .padding(.vertical, 8)
        .padding(.horizontal, 8)
        .hoverableRow()
        .onTapGesture { onSelect() }
        .help("点击查看详细设置")
    }
}
