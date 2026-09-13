import SwiftUI
import UniformTypeIdentifiers

struct ScriptRunnerEditorSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(DaemonController.self) private var controller

    @State private var kind = "chat"
    @State private var source = ""
    @State private var preview: RunnerScriptPreview?
    @State private var dependencyPresets: [RunnerDependencyPreset] = []
    @State private var dependencyPresetID = "stdlib"
    @State private var dependencyInput = ""
    @State private var isLoadingTemplate = false
    @State private var isApplyingDependencies = false
    @State private var isInspecting = false
    @State private var isCreating = false
    @State private var errorMessage: String?
    @State private var showRestartPrompt = false

    // 模板按能力缓存；loadedKind 记录编辑器当前内容对应的模板能力，
    // 两者共同判定“用户已编辑”，覆盖类操作（切换能力、导入文件）据此请求确认。
    @State private var templateCache: [String: String] = [:]
    @State private var loadedKind: String?
    @State private var importHint: String?
    @State private var showImporter = false
    @State private var pendingImport: String?
    @State private var showImportOverwriteConfirmation = false
    @State private var pendingKind: String?
    @State private var showKindSwitchConfirmation = false
    @State private var promptCopied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("新建单文件 Runner")
                .font(.headline)

            Picker("能力", selection: $kind) {
                Text("对话 · Chat").tag("chat")
                Text("语音识别 · STT").tag("stt")
                Text("语音合成 · TTS").tag("tts")
            }
            .pickerStyle(.segmented)
            .disabled(isLoadingTemplate || isApplyingDependencies || isCreating)

            HStack {
                Picker("推理库", selection: $dependencyPresetID) {
                    ForEach(dependencyPresets) { preset in
                        Text(preset.title).tag(preset.id)
                    }
                }
                .frame(width: 210)

                TextField("可粘贴：pip install package-a package-b", text: $dependencyInput)
                    .textFieldStyle(.roundedBorder)

                Button(isApplyingDependencies ? "应用中…" : "应用依赖", action: applyDependencies)
                    .disabled(dependencyPresets.isEmpty || isApplyingDependencies || isCreating)
            }

            Text("只填写代码直接使用的推理库；uv 会自动解析并锁定完整依赖树。")
                .font(.footnote)
                .foregroundStyle(.secondary)

            HStack {
                Button("导入 .py 文件…") {
                    errorMessage = nil
                    showImporter = true
                }
                .disabled(isLoadingTemplate || isApplyingDependencies || isCreating)

                Button(promptCopied ? "已复制" : "复制 AI Prompt", action: copyAIPrompt)
                    .disabled(isLoadingTemplate || isCreating)
            }

            RunnerScriptTextEditor(text: $source)
                .overlay {
                    RoundedRectangle(cornerRadius: Theme.Radius.control)
                        .stroke(Color(nsColor: .separatorColor))
                }
                .disabled(isLoadingTemplate || isCreating)

            if let importHint {
                Text(importHint)
                    .font(.footnote)
                    .foregroundStyle(.orange)
            }

            if let preview {
                RunnerScriptPreviewPanel(preview: preview)
            }

            if let errorMessage {
                ErrorBanner(text: errorMessage)
            }

            HStack {
                Text("点击“信任并添加”后会联网安装依赖并导入源码验证；代码以 aiworkd 当前用户权限运行。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 0)
                Button("取消", role: .cancel, action: dismiss.callAsFunction)
                    .keyboardShortcut(.cancelAction)
                Button(isInspecting ? "检查中…" : "检查", action: inspect)
                    .disabled(source.isEmpty || isApplyingDependencies || isInspecting || isCreating)
                Button(isCreating ? "创建中…" : "信任并添加", action: create)
                    .buttonStyle(ProminentButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .disabled(preview == nil || isApplyingDependencies || isCreating || isInspecting)
            }
        }
        .padding(20)
        .frame(minWidth: 720, minHeight: 640)
        .task {
            await loadTemplate(for: kind)
            await loadDependencyPresets()
        }
        .onChange(of: kind) { _, newKind in
            guard newKind != loadedKind else { return }
            if Self.isEditorContentUserModified(
                source: source,
                loadedTemplate: loadedKind.flatMap { templateCache[$0] }
            ) {
                pendingKind = newKind
                showKindSwitchConfirmation = true
            } else {
                Task { await loadTemplate(for: newKind) }
            }
        }
        .onChange(of: source) { _, newSource in
            preview = nil
            errorMessage = nil
            // 用户补上元数据块后提示自动消失；已有提示在普通编辑时保持。
            if Self.hasPEP723MetadataBlock(newSource) {
                importHint = nil
            }
        }
        .fileImporter(
            isPresented: $showImporter,
            allowedContentTypes: [Self.pythonSourceUTType],
            allowsMultipleSelection: false
        ) { result in
            switch result {
            case .success(let urls):
                guard let url = urls.first else { return }
                importPythonFile(at: url)
            case .failure(let error):
                errorMessage = "选择文件失败：\(DaemonController.message(for: error))"
            }
        }
        .confirmationDialog(
            "导入将覆盖编辑器当前内容",
            isPresented: $showImportOverwriteConfirmation,
            titleVisibility: .visible
        ) {
            Button("覆盖", role: .destructive) {
                if let pendingImport {
                    adoptImportedSource(pendingImport)
                }
                pendingImport = nil
            }
            Button("取消", role: .cancel) {
                pendingImport = nil
            }
        } message: {
            Text("编辑器中的修改不会被保存。")
        }
        .confirmationDialog(
            "切换能力会覆盖编辑器当前内容",
            isPresented: $showKindSwitchConfirmation,
            titleVisibility: .visible
        ) {
            Button("覆盖并切换") {
                guard let pendingKind else { return }
                Task { await loadTemplate(for: pendingKind) }
            }
            Button("取消", role: .cancel) {
                if let loadedKind {
                    kind = loadedKind
                }
            }
        } message: {
            Text("编辑器中的修改不会被保存。")
        }
        .confirmationDialog(
            "Runner 已添加，重启 aiworkd 后生效",
            isPresented: $showRestartPrompt,
            titleVisibility: .visible
        ) {
            Button("立即重启") {
                controller.restartDaemon()
                dismiss()
            }
            Button("稍后重启", action: dismiss.callAsFunction)
        } message: {
            Text("重启会停止已加载模型和当前推理任务。")
        }
    }

    private func loadTemplate(for requestedKind: String) async {
        isLoadingTemplate = true
        defer { isLoadingTemplate = false }
        if let cached = templateCache[requestedKind] {
            applyTemplate(for: requestedKind, template: cached)
            return
        }
        do {
            let template = try await controller.api.runnerScriptTemplate(kind: requestedKind)
            templateCache[requestedKind] = template
            guard requestedKind == kind else { return }
            applyTemplate(for: requestedKind, template: template)
        } catch {
            errorMessage = "模板加载失败：\(DaemonController.message(for: error))"
        }
    }

    private func applyTemplate(for requestedKind: String, template: String) {
        source = template
        loadedKind = requestedKind
        importHint = nil
    }

    private func importPythonFile(at url: URL) {
        let scoped = url.startAccessingSecurityScopedResource()
        defer { if scoped { url.stopAccessingSecurityScopedResource() } }
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            errorMessage = "读取文件失败：\(DaemonController.message(for: error))"
            return
        }
        guard !data.isEmpty else {
            errorMessage = "导入失败：文件为空。"
            return
        }
        guard data.count <= Self.maxSourceBytes else {
            errorMessage = "导入失败：文件超过 daemon 源码上限（1 MiB）。"
            return
        }
        guard let text = String(data: data, encoding: .utf8) else {
            errorMessage = "导入失败：文件不是有效的 UTF-8 文本。"
            return
        }
        if Self.isEditorContentUserModified(
            source: source,
            loadedTemplate: loadedKind.flatMap { templateCache[$0] }
        ) {
            pendingImport = text
            showImportOverwriteConfirmation = true
        } else {
            adoptImportedSource(text)
        }
    }

    private func adoptImportedSource(_ text: String) {
        source = text
        importHint = Self.hasPEP723MetadataBlock(text)
            ? nil
            : "未检测到 PEP 723 元数据块，点“检查”会失败；可点“复制 AI Prompt”生成完整文件后再导入。"
    }

    private func copyAIPrompt() {
        let requestedKind = kind
        errorMessage = nil
        Task {
            do {
                let template: String
                if let cached = templateCache[requestedKind] {
                    template = cached
                } else {
                    template = try await controller.api.runnerScriptTemplate(kind: requestedKind)
                    templateCache[requestedKind] = template
                }
                guard requestedKind == kind else { return }
                let prompt = ScriptRunnerAIPrompt.makeAIPrompt(kind: requestedKind, template: template)
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(prompt, forType: .string)
                promptCopied = true
                try? await Task.sleep(nanoseconds: 1_500_000_000)
                promptCopied = false
            } catch {
                guard requestedKind == kind else { return }
                errorMessage = "模板加载失败：\(DaemonController.message(for: error))"
            }
        }
    }

    private func loadDependencyPresets() async {
        do {
            dependencyPresets = try await controller.api.runnerDependencyPresets()
        } catch {
            errorMessage = "推理库预设加载失败：\(DaemonController.message(for: error))"
        }
    }

    private func applyDependencies() {
        let originalSource = source
        let requestedPreset = dependencyPresetID
        let requestedInput = dependencyInput
        isApplyingDependencies = true
        errorMessage = nil
        Task {
            defer { isApplyingDependencies = false }
            do {
                let dependencies = try await controller.api.normalizeRunnerDependencies(
                    preset: requestedPreset,
                    input: requestedInput
                )
                guard originalSource == source else { return }
                guard let updated = Self.replacingDependencies(in: originalSource, with: dependencies) else {
                    errorMessage = "模板中的 dependencies 必须保留为单行配置。"
                    return
                }
                source = updated
            } catch {
                errorMessage = "依赖解析失败：\(DaemonController.message(for: error))"
            }
        }
    }

    // daemon script.rs 的 MAX_SOURCE_BYTES。
    static let maxSourceBytes = 1_048_576

    static let pythonSourceUTType = UTType(filenameExtension: "py") ?? .plainText

    /// 编辑器内容是否已被用户改动：非空且不等于已加载模板。
    /// 模板基线缺失（首次加载失败）时，任何非空内容都按已改动处理。
    static func isEditorContentUserModified(source: String, loadedTemplate: String?) -> Bool {
        guard !source.isEmpty else { return false }
        guard let loadedTemplate else { return true }
        return source != loadedTemplate
    }

    /// 与 daemon script.rs 的解析规则一致：块首行必须是 `# /// script`。
    static func hasPEP723MetadataBlock(_ source: String) -> Bool {
        source.range(of: #"(?m)^# /// script$"#, options: .regularExpression) != nil
    }

    static func replacingDependencies(in source: String, with dependencies: [String]) -> String? {
        guard let data = try? JSONSerialization.data(withJSONObject: dependencies),
              let value = String(data: data, encoding: .utf8),
              let range = source.range(
                  of: #"(?m)^# dependencies = \[[^\r\n]*\]$"#,
                  options: .regularExpression
              ) else { return nil }
        var result = source
        result.replaceSubrange(range, with: "# dependencies = \(value)")
        return result
    }

    private func inspect() {
        let inspectedSource = source
        isInspecting = true
        errorMessage = nil
        Task {
            defer { isInspecting = false }
            do {
                let result = try await controller.api.inspectRunnerScript(source: inspectedSource)
                guard inspectedSource == source else { return }
                preview = result
            } catch {
                errorMessage = "检查失败：\(DaemonController.message(for: error))"
            }
        }
    }

    private func create() {
        guard let preview else { return }
        isCreating = true
        errorMessage = nil
        Task {
            defer { isCreating = false }
            do {
                showRestartPrompt = try await controller.api.createRunnerScript(
                    source: source,
                    expectedDigest: preview.sourceDigest
                )
            } catch {
                errorMessage = "创建失败：\(DaemonController.message(for: error))"
            }
        }
    }
}
