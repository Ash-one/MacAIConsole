import SwiftUI

struct ModelsView: View {
    @Environment(DaemonController.self) private var controller
    @State private var repoModels: [RepoModel] = []
    @State private var showingAddSheet = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if controller.phase != .online {
                    OfflineHint { controller.startDaemon() }
                }
                if let error = controller.lastError {
                    ErrorBanner(text: error)
                }
                registeredSection
                repoSection
            }
            .padding(24)
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
                    rescanRepo()
                    Task { try? await controller.refresh() }
                } label: {
                    Label("刷新", systemImage: "arrow.clockwise")
                }
            }
        }
        .sheet(isPresented: $showingAddSheet, onDismiss: { rescanRepo() }) {
            AddModelSheet()
        }
        .task {
            controller.bootstrapIfNeeded()
            rescanRepo()
        }
    }

    /// 每次都重新扫描仓库目录：放入文件的即刻可见。
    private func rescanRepo() {
        repoModels = ModelRepository.scan()
    }

    private var loadedIDs: Set<String> {
        Set((controller.info?.loadedModels ?? []).map(\.id))
    }

    /// 已注册的模型 ID（daemon 注册表里有记录就算，不论是否加载）。
    private var registeredIDs: Set<String> {
        Set(controller.registeredModels.map(\.id))
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

    private var registeredSection: some View {
        GroupBox {
            if controller.registeredModels.isEmpty {
                Text(controller.phase == .online ? "注册表为空" : "守护进程离线，暂无数据")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity, minHeight: 44)
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
                                if entry.id != entries(ofType: type).last?.id { Divider().opacity(0.25) }
                            }
                        }
                    }
                }
            }
        } label: {
            Text("模型注册 ID")
                .font(.title3.weight(.semibold))
                .padding(.bottom, 8)
        }
    }

    private var repoSection: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: 6) {
                    Text("路径：\(ModelRepository.baseURL.path)")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .help(ModelRepository.baseURL.path)
                    Button {
                        let pasteboard = NSPasteboard.general
                        pasteboard.clearContents()
                        pasteboard.setString(ModelRepository.baseURL.path, forType: .string)
                    } label: {
                        Image(systemName: "doc.on.doc")
                            .font(.caption2)
                    }
                    .buttonStyle(.borderless)
                    .foregroundStyle(.tertiary)
                    .help("复制完整路径")
                    .padding(.trailing, 2)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 4)
                .padding(.vertical, 4)

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
                                let allRegistered = entries.allSatisfy { registeredIDs.contains($0.modelID) }
                                ModelTypeSection(type: type, title: typeLabel(type), count: entries.count, dimmed: allRegistered) {
                                    ForEach(entries) { model in
                                        RepoModelRow(
                                            model: model,
                                            // 仓库行只关心「是否已注册」，不同步展示运行时加载状态。
                                            isLoaded: false,
                                            isRegistered: registeredIDs.contains(model.modelID)
                                        )
                                        if model.id != entries.last?.id { Divider().opacity(0.25) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            .padding(.horizontal, 4)
            .padding(.bottom, 6)
        } label: {
            Text("模型仓库（Models/llm · tts · stt）")
                .font(.title3.weight(.semibold))
                .padding(.bottom, 8)
        }
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
                Text("\(count) 个")
                    .font(.caption2.weight(.medium))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2)
                    .background(Capsule().fill(Color.primary.opacity(0.06)))
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

    @State private var isHovering = false

    var body: some View {
        HStack(spacing: 10) {
            ModelTypeIcon(type: entry.modelType)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 8) {
                    Text(entry.id)
                        .font(.body.weight(.semibold))
                    if isWorkerProcess {
                        Text("独立进程")
                            .font(.caption2.weight(.medium))
                            .foregroundStyle(Color.accentColor)
                            .padding(.horizontal, 8)
                            .padding(.vertical, 2)
                            .background(Capsule().fill(Color.accentColor.opacity(0.10)))
                    }
                }
                Text(entry.ownedBy)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
            }
            Spacer(minLength: 12)
            if isLoaded {
                HStack(spacing: 5) {
                    Circle()
                        .fill(.green)
                        .frame(width: 6, height: 6)
                        .shadow(color: .green.opacity(0.7), radius: 2.5)
                    Text(isWorkerProcess ? "运行中" : "已加载")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.green)
                }
                .help(isWorkerProcess ? "独立进程正在运行" : "模型已加载到内存")
                if controller.busyModelIDs.contains(entry.id) {
                    ProgressView()
                } else {
                    Button {
                        Task { await controller.unload(entry.id) }
                    } label: {
                        Image(systemName: isWorkerProcess ? "stop.fill" : "eject.fill")
                            .font(.callout.weight(.semibold))
                    }
                    .buttonStyle(.bordered)
                    .tint(Color(nsColor: .secondaryLabelColor))
                    .help(isWorkerProcess ? "停止独立进程" : "卸载")
                    .disabled(controller.phase != .online)
                }
            } else {
                if controller.busyModelIDs.contains(entry.id) {
                    ProgressView()
                } else {
                    Button {
                        Task { await controller.loadRegistered(entry.id) }
                    } label: {
                        Image(systemName: "play.fill")
                            .font(.callout.weight(.semibold))
                    }
                    .buttonStyle(.borderedProminent)
                    .help(isWorkerProcess ? "启动独立进程" : "加载")
                    .disabled(controller.phase != .online)
                }
            }
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 6)
        .contentShape(Rectangle())
        .background(isHovering ? Color.primary.opacity(0.05) : Color.clear)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .onHover { isHovering = $0 }
        .animation(.snappy(duration: 0.15), value: isHovering)
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

    @State private var contextDraft = ""
    @State private var isReloading = false
    @State private var isPreviewing = false
    @State private var isHovering = false

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
                Text(model.fileName)
                    .font(.body.weight(.semibold))
                    .foregroundStyle(isRegistered ? .secondary : .primary)
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
        .padding(.horizontal, 6)
        .contentShape(Rectangle())
        .background(isHovering ? Color.primary.opacity(0.05) : Color.clear)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .onHover { isHovering = $0 }
        .animation(.snappy(duration: 0.15), value: isHovering)
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

    /// 存储值 → 显示文本：能被 1024 整除时用 K 记法（4096 → "4K"），否则显示原始数值。
    private static func displayValue(_ tokens: Int) -> String {
        tokens % 1024 == 0 ? "\(tokens / 1024)K" : "\(tokens)"
    }

    /// 输入文本 → token 数：支持 K 记法（"4k" / "8K" / "0.5k"）与原始数值（"4096"）。
    private static func parseValue(_ text: String) -> Int? {
        let trimmed = text.trimmingCharacters(in: .whitespaces).lowercased()
        guard !trimmed.isEmpty else { return nil }
        if trimmed.hasSuffix("k") {
            let number = trimmed.dropLast()
            guard let value = Double(number), value > 0 else { return nil }
            return Int(value * 1024)
        }
        return Int(trimmed)
    }

    /// llm 的上下文长度编辑：改动了才出现「应用」，点击即保存并自动重载。
    private var contextEditor: some View {
        HStack(spacing: 4) {
            Text("上下文")
                .font(.caption)
                .foregroundStyle(.secondary)
            TextField("4K", text: $contextDraft)
                .font(.caption.monospacedDigit())
                .frame(width: 56)
                .multilineTextAlignment(.trailing)
                .onSubmit(applyContext)
            if contextChanged, parsedContext != nil {
                Button(isReloading ? "重载中…" : "应用") { applyContext() }
                    .buttonStyle(.borderedProminent)
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
                Circle()
                    .fill(.green)
                    .frame(width: 6, height: 6)
                    .shadow(color: .green.opacity(0.7), radius: 2.5)
                Text("已加载")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.green)
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
            .tint(isRegistered ? Color(nsColor: .secondaryLabelColor) : Color.accentColor)
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
                modelType: isLLM ? nil : model.modelType
            )
        } catch {
            controller.lastError = "加载失败：\(DaemonController.message(for: error))"
        }
    }
}