import SwiftUI

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

            RunnerScriptTextEditor(text: $source)
                .overlay {
                    RoundedRectangle(cornerRadius: Theme.Radius.control)
                        .stroke(Color(nsColor: .separatorColor))
                }
                .disabled(isLoadingTemplate || isCreating)

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
            await loadTemplate()
            await loadDependencyPresets()
        }
        .onChange(of: kind) { _, _ in
            Task { await loadTemplate() }
        }
        .onChange(of: source) { _, _ in
            preview = nil
            errorMessage = nil
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

    private func loadTemplate() async {
        let requestedKind = kind
        isLoadingTemplate = true
        defer { isLoadingTemplate = false }
        do {
            let template = try await controller.api.runnerScriptTemplate(kind: requestedKind)
            guard requestedKind == kind else { return }
            source = template
        } catch {
            errorMessage = "模板加载失败：\(DaemonController.message(for: error))"
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
