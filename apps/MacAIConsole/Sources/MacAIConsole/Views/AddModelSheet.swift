import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// 添加模型：既可导入本地单文件，也可把公开远端仓库下载到统一模型目录。
struct AddModelSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(DaemonController.self) private var controller

    @State private var mode = Mode.local
    @State private var pickedURL: URL?
    @State private var modelType = "llm"
    @State private var modelID = ""
    @State private var idEdited = false
    @State private var remoteSource = RemoteModelSource.huggingface
    @State private var remoteInput = ""
    @State private var inspection: RemoteModelInspection?
    @State private var selectedPaths = Set<String>()
    @State private var isInspecting = false
    @State private var isWorking = false
    @State private var errorMessage: String?

    private let types: [(id: String, label: String, ext: String)] = [
        ("llm", "对话 · LLM", "gguf"),
        ("stt", "语音识别 · STT", "bin"),
    ]
    private let remoteTypes: [(id: String, label: String)] = [
        ("llm", "对话 · LLM"),
        ("stt", "语音识别 · STT"),
        ("tts", "语音合成 · TTS"),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("添加模型到仓库")
                .font(.headline)

            Picker("来源", selection: $mode) {
                ForEach(Mode.allCases) { mode in
                    Text(mode.title).tag(mode)
                }
            }
            .pickerStyle(.segmented)

            if mode == .local {
                localForm
            } else {
                remoteForm
            }

            if let errorMessage {
                ErrorBanner(text: errorMessage)
            }

            HStack {
                Spacer(minLength: 0)
                Button("取消", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                if mode == .local {
                    Button(isWorking ? "加载中…" : "添加并加载") { submitLocal() }
                        .buttonStyle(ProminentButtonStyle())
                        .keyboardShortcut(.defaultAction)
                        .disabled(!localIsValid || isWorking)
                } else {
                    Button(isWorking ? "下载中…" : "下载到模型仓库") { downloadRemote() }
                        .buttonStyle(ProminentButtonStyle())
                        .keyboardShortcut(.defaultAction)
                        .disabled(!remoteIsValid || isWorking || isInspecting)
                }
            }
        }
        .padding(20)
        .frame(width: mode == .local ? 500 : 620)
        .onChange(of: mode) { _, newMode in
            if newMode == .local && modelType == "tts" { modelType = "llm" }
        }
        .onChange(of: modelType) { _, _ in selectDefaults() }
    }

    private var localForm: some View {
        Group {
            Form {
                Picker("类型", selection: $modelType) {
                    ForEach(types, id: \.id) { type in
                        Text(type.label).tag(type.id)
                    }
                }

                HStack {
                    Text(pickedURL?.path ?? "请选择模型文件")
                        .font(.callout)
                        .foregroundStyle(pickedURL == nil ? .secondary : .primary)
                        .lineLimit(1)
                    Spacer()
                    Button("浏览…") { pickFile() }
                }

                TextField("模型 ID（供 API 调用）", text: $modelID)
                    .onChange(of: modelID) { _, _ in idEdited = true }
            }
            .formStyle(.grouped)

            Text("文件会拷入仓库 Models/\(modelType)/，随后由 daemon 选择匹配的 Runner 并注册加载。")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    private var remoteForm: some View {
        VStack(alignment: .leading, spacing: 12) {
            Form {
                Picker("平台", selection: $remoteSource) {
                    ForEach(RemoteModelSource.allCases) { source in
                        Text(source.title).tag(source)
                    }
                }
                Picker("类型", selection: $modelType) {
                    ForEach(remoteTypes, id: \.id) { type in
                        Text(type.label).tag(type.id)
                    }
                }
                HStack {
                    TextField("owner/repo 或完整模型 URL", text: $remoteInput)
                        .onSubmit { inspectRemote() }
                    Button(isInspecting ? "获取中…" : "获取文件列表") { inspectRemote() }
                        .disabled(remoteInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || isInspecting || isWorking)
                }
            }
            .formStyle(.grouped)

            if isInspecting {
                ProgressView("正在读取远端文件清单…")
                    .frame(maxWidth: .infinity, alignment: .center)
            } else if let inspection {
                HStack {
                    Text("\(inspection.source.title) · \(inspection.repo)")
                        .font(.callout)
                        .bold()
                    Spacer()
                    if !singleFileMode {
                        Button("全选") { selectedPaths = Set(displayFiles.map(\.path)) }
                        Button("清空") { selectedPaths.removeAll() }
                    }
                }

                List(displayFiles) { file in
                    Button {
                        toggle(file)
                    } label: {
                        HStack(spacing: 10) {
                            Image(systemName: selectedPaths.contains(file.path)
                                ? (singleFileMode ? "largecircle.fill.circle" : "checkmark.square.fill")
                                : (singleFileMode ? "circle" : "square"))
                                .foregroundStyle(selectedPaths.contains(file.path) ? Color.accentColor : .secondary)
                            Text(file.path)
                                .lineLimit(1)
                                .truncationMode(.middle)
                            Spacer()
                            Text(Format.bytes(file.sizeBytes))
                                .foregroundStyle(.secondary)
                        }
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("\(selectedPaths.contains(file.path) ? "已选择" : "未选择") \(file.path)，\(Format.bytes(file.sizeBytes))")
                }
                .frame(minHeight: 200, maxHeight: 300)

                Text("已选 \(selectedPaths.count) 个文件，共 \(Format.bytes(selectedSize))。下载完成后只刷新本地仓库，不会注册或加载模型。")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var expectedExtension: String {
        types.first { $0.id == modelType }?.ext ?? "gguf"
    }

    private var localIsValid: Bool {
        guard let pickedURL, !modelID.isEmpty else { return false }
        return pickedURL.pathExtension.lowercased() == expectedExtension
            && modelID.allSatisfy { $0.isLetter || $0.isNumber || "._-".contains($0) }
    }

    private var singleFileCandidates: [RemoteModelFile] {
        guard let inspection else { return [] }
        return RemoteModelSelection.singleFileCandidates(modelType: modelType, files: inspection.files)
    }

    private var singleFileMode: Bool { !singleFileCandidates.isEmpty }

    private var displayFiles: [RemoteModelFile] {
        singleFileMode ? singleFileCandidates : (inspection?.files ?? [])
    }

    private var selectedFiles: [RemoteModelFile] {
        displayFiles.filter { selectedPaths.contains($0.path) }
    }

    private var selectedSize: UInt64 {
        selectedFiles.reduce(0) { total, file in
            let (sum, overflow) = total.addingReportingOverflow(file.sizeBytes)
            return overflow ? .max : sum
        }
    }

    private var remoteIsValid: Bool {
        guard let inspection, !selectedFiles.isEmpty,
              let reference = try? RemoteModelReference.parse(remoteInput, selectedSource: remoteSource),
              reference.source == inspection.source,
              reference.repo == inspection.repo else { return false }
        return !singleFileMode || selectedFiles.count == 1
    }

    private func pickFile() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        if let type = UTType(filenameExtension: expectedExtension) {
            panel.allowedContentTypes = [type]
        }
        panel.directoryURL = pickedURL?.deletingLastPathComponent()
        if panel.runModal() == .OK, let url = panel.url {
            pickedURL = url
            if !idEdited { modelID = url.deletingPathExtension().lastPathComponent }
        }
    }

    private func inspectRemote() {
        errorMessage = nil
        isInspecting = true
        Task {
            do {
                let reference = try RemoteModelReference.parse(remoteInput, selectedSource: remoteSource)
                let result = try await controller.api.inspectRemoteModel(source: reference.source, repo: reference.repo)
                remoteSource = result.source
                remoteInput = result.repo
                inspection = result
                selectDefaults()
            } catch {
                inspection = nil
                selectedPaths.removeAll()
                errorMessage = DaemonController.message(for: error)
            }
            isInspecting = false
        }
    }

    private func selectDefaults() {
        guard inspection != nil else { return }
        let candidates = singleFileCandidates
        selectedPaths = candidates.isEmpty
            ? Set(displayFiles.map(\.path))
            : Set(candidates.count == 1 ? candidates.map(\.path) : [])
    }

    private func toggle(_ file: RemoteModelFile) {
        if singleFileMode {
            selectedPaths = [file.path]
        } else if selectedPaths.contains(file.path) {
            selectedPaths.remove(file.path)
        } else {
            selectedPaths.insert(file.path)
        }
    }

    private func submitLocal() {
        guard let source = pickedURL else { return }
        isWorking = true
        errorMessage = nil
        let type = modelType
        let id = modelID
        Task {
            do {
                if type == "llm" { _ = try await ModelRepository.inspectGGUFInBackground(at: source) }
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

    private func downloadRemote() {
        guard let inspection else { return }
        isWorking = true
        errorMessage = nil
        Task {
            do {
                _ = try await controller.api.pullRemoteModel(
                    inspection,
                    modelType: modelType,
                    files: selectedFiles,
                    singleFile: singleFileMode
                )
                dismiss()
            } catch {
                errorMessage = DaemonController.message(for: error)
                isWorking = false
            }
        }
    }

    private enum Mode: String, CaseIterable, Identifiable {
        case local
        case remote

        var id: String { rawValue }
        var title: String { self == .local ? "本地文件" : "在线仓库" }
    }
}
