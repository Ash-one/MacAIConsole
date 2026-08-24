import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// 添加模型：选择文件与类型，拷入统一仓库对应文件夹，并立即注册加载。
struct AddModelSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(DaemonController.self) private var controller

    @State private var pickedURL: URL?
    @State private var modelType = "llm"
    @State private var modelID = ""
    @State private var isWorking = false
    @State private var errorMessage: String?
    @State private var idEdited = false

    /// llm → .gguf（llama.cpp），stt → .bin（whisper.cpp）；tts 走系统 macos-say，暂无文件型模型。
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

                TextField("模型 ID（供 API 调用）", text: $modelID)
                    .onChange(of: modelID) { _, _ in idEdited = true }
            }
            .formStyle(.grouped)

            Text("文件会拷入仓库 Models/\(modelType)/，随后立即注册加载；之后把同类文件放进该文件夹即可在列表中出现。")
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
                    .buttonStyle(.borderedProminent)
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
        return pickedURL.pathExtension.lowercased() == expectedExtension
            && modelID.allSatisfy { $0.isLetter || $0.isNumber || $0 == "." || $0 == "_" || $0 == "-" }
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

    private func submit() {
        guard let source = pickedURL else { return }
        isWorking = true
        errorMessage = nil
        let type = modelType
        let id = modelID
        Task {
            do {
                // 先拷入仓库（失败则中止，不碰 daemon），再注册加载。
                let imported = try ModelRepository.importFile(at: source, type: type)
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
}
