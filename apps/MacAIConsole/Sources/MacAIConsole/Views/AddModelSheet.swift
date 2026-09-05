import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// 添加模型：选择文件与类型，拷入统一仓库对应文件夹，并立即注册加载。
struct AddModelSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(DaemonController.self) private var controller

    @State private var pickedURL: URL?
    @State private var pickedCoreMLURL: URL?
    @State private var modelType = "llm"
    @State private var modelID = ""
    @State private var isWorking = false
    @State private var errorMessage: String?
    @State private var idEdited = false

    /// llm → .gguf（llama.cpp Runner），stt → .bin（whisper.cpp Runner）；目录模型（Kokoro /
    /// Qwen3-ASR / Qwen3-TTS / sherpa-onnx）由 ModelRepository 扫描并绑定
    /// org.macai.* Runner。mock 与 macos-say 仅测试/CLI 兜底，无 GUI 注册入口。
    private let types: [(id: String, label: String, ext: String)] = [
        ("llm", "对话 · LLM", "gguf"),
        ("stt", "语音识别 · STT", "bin"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("添加模型到仓库")
                .font(.headline)

            Form {
                Picker("类型", selection: $modelType) {
                    ForEach(types, id: \.id) { type in
                        Text(type.label).tag(type.id)
                    }
                }

                HStack {
                    TextField("文件路径", text: Binding(
                        get: { pickedURL?.path ?? "" },
                        set: { if $0.isEmpty { pickedURL = nil } }
                    ))
                    .font(.callout)
                    Button("浏览…") { pickFile() }
                }

                if modelType == "stt" {
                    HStack {
                        TextField("Core ML 编译模型（可选）", text: Binding(
                            get: { pickedCoreMLURL?.path ?? "" },
                            set: { if $0.isEmpty { pickedCoreMLURL = nil } }
                        ))
                        .font(.callout)
                        Button("浏览…") { pickCoreMLDirectory() }
                    }
                    Text("选择 .mlmodelc 目录后会与 .bin 一起导入；未选择时自动回退 Metal。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }

                TextField("模型 ID（供 API 调用）", text: $modelID)
                    .onChange(of: modelID) { _, _ in idEdited = true }
            }
            .formStyle(.grouped)
            .onChange(of: modelType) { _, newValue in
                if newValue != "stt" {
                    pickedCoreMLURL = nil
                }
            }

            Text("文件会拷入仓库 Models/\(modelType)/，随后立即注册加载；STT 的可选 .mlmodelc 会与 .bin 放在一起。")
                .font(.footnote)
                .foregroundStyle(.secondary)

            if let errorMessage {
                ErrorBanner(text: errorMessage)
            }

            HStack {
                Spacer(minLength: 0)
                Button("取消", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(isWorking ? "加载中…" : "添加并加载") { submit() }
                    .buttonStyle(ProminentButtonStyle())
                    .keyboardShortcut(.defaultAction)
                    .disabled(!isValid || isWorking)
            }
        }
        .padding(20)
        .frame(width: 500)
    }

    private var expectedExtension: String {
        types.first { $0.id == modelType }?.ext ?? "gguf"
    }

    private var isValid: Bool {
        guard let pickedURL, !modelID.isEmpty else { return false }
        guard pickedURL.pathExtension.lowercased() == expectedExtension,
              modelID.allSatisfy({ $0.isLetter || $0.isNumber || $0 == "." || $0 == "_" || $0 == "-" }) else {
            return false
        }
        guard let pickedCoreMLURL else { return true }
        return modelType == "stt"
            && pickedCoreMLURL.pathExtension.lowercased() == "mlmodelc"
            && FileManager.default.fileExists(atPath: pickedCoreMLURL.path)
    }

    private func pickFile() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        if let uttype = UTType(filenameExtension: expectedExtension) {
            panel.allowedContentTypes = [uttype]
        }
        panel.directoryURL = pickedURL?.deletingLastPathComponent()
        if panel.runModal() == .OK, let url = panel.url {
            pickedURL = url
            if !idEdited {
                modelID = url.deletingPathExtension().lastPathComponent
            }
        }
    }

    private func pickCoreMLDirectory() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.directoryURL = pickedCoreMLURL?.deletingLastPathComponent()
        if panel.runModal() == .OK, let url = panel.url,
           url.pathExtension.lowercased() == "mlmodelc" {
            pickedCoreMLURL = url
        }
    }

    private func submit() {
        guard let source = pickedURL else { return }
        isWorking = true
        errorMessage = nil
        let type = modelType
        let id = modelID
        Task {
            do {
                if type == "llm" {
                    _ = try await ModelRepository.inspectGGUFInBackground(at: source)
                }
                // 先拷入仓库（失败则中止，不碰 daemon），再注册加载。
                let imported = try ModelRepository.importFile(at: source, type: type)
                if type == "stt", let coreMLSource = pickedCoreMLURL {
                    _ = try ModelRepository.importResource(
                        at: coreMLSource,
                        type: type,
                        destinationName: coreMLResourceName(for: imported)
                    )
                }
                try await controller.registerAndLoad(
                    path: imported.path,
                    id: id,
                    contextLength: 4096,
                    keepAlive: nil,
                    modelType: type
                )
                dismiss()
            } catch {
                errorMessage = DaemonController.message(for: error)
                isWorking = false
            }
        }
    }

    /// whisper.cpp 会从 .bin 自动寻找同目录的 `<stem>-encoder.mlmodelc`。
    private func coreMLResourceName(for modelURL: URL) -> String {
        var stem = modelURL.deletingPathExtension().lastPathComponent
        let suffix = Array(stem.suffix(5))
        if suffix.count == 5, suffix[0] == "-", suffix[1] == "q", suffix[3] == "_" {
            stem.removeLast(5)
        }
        return "\(stem)-encoder.mlmodelc"
    }
}
