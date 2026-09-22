import AVFoundation
import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// 添加自定义参考音频音色 Sheet。
///
/// 允许用户选择或拖拽本地音频（WAV / MP3 / M4A / FLAC 等），通过 daemon
/// 的 `POST /api/models/{id}/voices` 上传并归一化为 PCM WAV，注册为该模型的参考音色。
struct CustomVoiceSheet: View {
    let modelID: String
    var onVoiceUploaded: ((String) -> Void)?

    @Environment(\.dismiss) private var dismiss
    @Environment(DaemonController.self) private var controller

    @State private var voiceName = ""
    @State private var pickedURL: URL?
    @State private var pickedData: Data?
    @State private var setAsDefault = true
    @State private var isUploading = false
    @State private var isPlayingPreview = false
    @State private var errorMessage: String?
    @State private var isTargetedForDrop = false
    @State private var audioPlayer: AVAudioPlayer?
    @State private var playbackDelegate: SheetPlaybackDelegate?

    private static let supportedExtensions = ["wav", "mp3", "m4a", "flac", "ogg", "aac", "alac"]

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            headerSection
            formSection
            dropOrFileSection

            if let errorMessage {
                ErrorBanner(text: errorMessage)
            }

            Spacer(minLength: 0)

            footerButtons
        }
        .padding(Theme.Space.page)
        .frame(width: 480, height: 430)
        .onDisappear {
            stopPreview()
        }
    }

    // MARK: - 视图分区

    private var headerSection: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("添加自定义参考音频")
                .font(.headline)
            Text("上传一段 3~15 秒的清晰参考音频（支持 WAV、MP3、M4A、FLAC 等）。后台会自动转码为规范 PCM WAV 并作为模型音色保存。")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var formSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Text("音色名称:")
                    .font(.callout.weight(.medium))
                    .frame(width: 70, alignment: .trailing)
                TextField("如：我的声音 / 客服女声", text: $voiceName)
                    .textFieldStyle(.roundedBorder)
            }

            HStack(spacing: 8) {
                Spacer().frame(width: 70)
                Toggle("保存后设为默认音色", isOn: $setAsDefault)
                    .toggleStyle(.checkbox)
                    .font(.caption)
            }
        }
    }

    private var dropOrFileSection: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 10)
                .strokeBorder(
                    isTargetedForDrop ? Theme.accent : Color.secondary.opacity(0.3),
                    style: StrokeStyle(lineWidth: 1.5, dash: [6, 4])
                )
                .background(
                    RoundedRectangle(cornerRadius: 10)
                        .fill(isTargetedForDrop ? Theme.accent.opacity(0.08) : Color.primary.opacity(0.02))
                )

            if let pickedURL, let pickedData {
                selectedFileView(url: pickedURL, data: pickedData)
            } else {
                emptyDropView
            }
        }
        .frame(height: 140)
        .onDrop(of: [.audio, .fileURL], isTargeted: $isTargetedForDrop) { providers in
            handleDrop(providers)
        }
    }

    private var emptyDropView: some View {
        VStack(spacing: 8) {
            Image(systemName: "waveform.badge.plus")
                .font(.system(size: 28))
                .foregroundStyle(isTargetedForDrop ? Theme.accent : .secondary)

            Text("拖拽音频文件到此处，或点击浏览选择")
                .font(.callout)
                .foregroundStyle(.secondary)

            Button("选择音频文件…") {
                pickFile()
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
    }

    private func selectedFileView(url: URL, data: Data) -> some View {
        VStack(spacing: 10) {
            HStack(spacing: 12) {
                Image(systemName: "waveform.circle.fill")
                    .font(.system(size: 30))
                    .foregroundStyle(Theme.accent)

                VStack(alignment: .leading, spacing: 2) {
                    Text(url.lastPathComponent)
                        .font(.callout.weight(.medium))
                        .lineLimit(1)
                    Text(ByteCountFormatter.string(fromByteCount: Int64(data.count), countStyle: .file))
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }

                Spacer()

                Button {
                    togglePreview()
                } label: {
                    Label(isPlayingPreview ? "停止" : "试听", systemImage: isPlayingPreview ? "stop.fill" : "play.fill")
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
            }
            .padding(.horizontal, 16)

            HStack {
                Spacer()
                Button("更换文件…") {
                    pickFile()
                }
                .buttonStyle(.plain)
                .font(.caption)
                .foregroundStyle(Theme.accent)
            }
            .padding(.horizontal, 16)
        }
    }

    private var footerButtons: some View {
        HStack {
            Spacer()
            Button("取消", role: .cancel) {
                dismiss()
            }
            .keyboardShortcut(.cancelAction)

            Button(isUploading ? "正在转码上传…" : "保存音色") {
                uploadVoice()
            }
            .buttonStyle(ProminentButtonStyle())
            .keyboardShortcut(.defaultAction)
            .disabled(!isFormValid || isUploading)
        }
    }

    private var isFormValid: Bool {
        !voiceName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && pickedData != nil
    }

    // MARK: - 交互操作

    private func pickFile() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.canChooseFiles = true
        var types: [UTType] = [.audio, .wav, .mp3]
        for ext in Self.supportedExtensions {
            if let type = UTType(filenameExtension: ext) {
                types.append(type)
            }
        }
        panel.allowedContentTypes = types
        if panel.runModal() == .OK, let url = panel.url {
            loadPickedFile(url: url)
        }
    }

    private func handleDrop(_ providers: [NSItemProvider]) -> Bool {
        guard let provider = providers.first else { return false }
        _ = provider.loadObject(ofClass: URL.self) { url, _ in
            guard let url else { return }
            Task { @MainActor in
                let ext = url.pathExtension.lowercased()
                if Self.supportedExtensions.contains(ext) {
                    loadPickedFile(url: url)
                } else {
                    errorMessage = "不支持的音频格式（支持：\(Self.supportedExtensions.joined(separator: ", "))）"
                }
            }
        }
        return true
    }

    private func loadPickedFile(url: URL) {
        stopPreview()
        errorMessage = nil
        do {
            let data = try Data(contentsOf: url)
            guard !data.isEmpty else {
                errorMessage = "音频文件内容为空"
                return
            }
            pickedURL = url
            pickedData = data
            if voiceName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                voiceName = url.deletingPathExtension().lastPathComponent
            }
        } catch {
            errorMessage = "无法读取音频文件: \(error.localizedDescription)"
        }
    }

    private func togglePreview() {
        if isPlayingPreview {
            stopPreview()
        } else if let data = pickedData {
            playPreview(data: data)
        }
    }

    private func playPreview(data: Data) {
        stopPreview()
        do {
            let player = try AVAudioPlayer(data: data)
            let delegate = SheetPlaybackDelegate {
                self.isPlayingPreview = false
            }
            player.delegate = delegate
            self.playbackDelegate = delegate
            self.audioPlayer = player
            player.prepareToPlay()
            if player.play() {
                isPlayingPreview = true
            }
        } catch {
            errorMessage = "本地音频试听失败: \(error.localizedDescription)"
        }
    }

    private func stopPreview() {
        audioPlayer?.stop()
        audioPlayer = nil
        playbackDelegate = nil
        isPlayingPreview = false
    }

    private func uploadVoice() {
        guard let data = pickedData, let url = pickedURL else { return }
        stopPreview()
        isUploading = true
        errorMessage = nil

        let name = voiceName.trimmingCharacters(in: .whitespacesAndNewlines)
        let fileName = url.lastPathComponent
        let defaultFlag = setAsDefault

        Task {
            do {
                let response = try await controller.api.uploadVoice(
                    modelID: modelID,
                    name: name,
                    audioData: data,
                    fileName: fileName,
                    setAsDefault: defaultFlag
                )
                onVoiceUploaded?(response.voice)
                dismiss()
            } catch {
                errorMessage = DaemonController.message(for: error)
                isUploading = false
            }
        }
    }
}

// MARK: - 试听播放 Delegate

private final class SheetPlaybackDelegate: NSObject, AVAudioPlayerDelegate {
    private let onFinish: () -> Void

    init(onFinish: @escaping () -> Void) {
        self.onFinish = onFinish
    }

    func audioPlayerDidFinishPlaying(_ player: AVAudioPlayer, successfully flag: Bool) {
        onFinish()
    }

    func audioPlayerDecodeErrorDidFinishPlaying(_ player: AVAudioPlayer, error: Error?) {
        onFinish()
    }
}
