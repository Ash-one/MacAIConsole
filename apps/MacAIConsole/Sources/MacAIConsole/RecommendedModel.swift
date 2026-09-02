import Foundation

/// 经过 MacAI Provider 实测的推荐模型。清单只包含运行所需文件，避免把示例音频等资源一并下载。
struct RecommendedModel: Identifiable, Hashable {
    let id: String
    let title: String
    let summary: String
    let modelType: String
    let provider: String
    let repository: String
    let files: [String]
    let directoryName: String?
    let estimatedSizeBytes: UInt64

    var repositoryURL: URL {
        URL(string: "https://huggingface.co/\(repository)")!
    }

    var localURL: URL {
        let folder = ModelRepository.folder(for: modelType)
        if let directoryName {
            return folder.appendingPathComponent(directoryName, isDirectory: true)
        }
        return folder.appendingPathComponent(files[0])
    }

    var downloadedURL: URL? {
        downloadedURL(in: ModelRepository.folder(for: modelType))
    }

    var isDownloaded: Bool {
        downloadedURL != nil
    }

    func downloadedURL(in repositoryFolder: URL) -> URL? {
        let fm = FileManager.default
        guard let directoryName else {
            let file = repositoryFolder.appendingPathComponent(files[0])
            var isDirectory: ObjCBool = false
            return fm.fileExists(atPath: file.path, isDirectory: &isDirectory) && !isDirectory.boolValue
                ? file
                : nil
        }

        let preferred = repositoryFolder.appendingPathComponent(directoryName, isDirectory: true)
        let siblings = (try? fm.contentsOfDirectory(
            at: repositoryFolder,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: .skipsHiddenFiles
        )) ?? []
        let candidates = [preferred] + siblings
            .filter { $0.standardizedFileURL != preferred.standardizedFileURL }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
        return candidates.first { containsAllFiles(at: $0) }
    }

    func matches(repositoryModel: RepoModel) -> Bool {
        guard repositoryModel.modelType == modelType else { return false }
        let url = URL(fileURLWithPath: repositoryModel.path)
        if directoryName != nil {
            return containsAllFiles(at: url)
        }
        return localURL.standardizedFileURL == url.standardizedFileURL
    }

    private func containsAllFiles(at root: URL) -> Bool {
        let fm = FileManager.default
        var rootIsDirectory: ObjCBool = false
        guard fm.fileExists(atPath: root.path, isDirectory: &rootIsDirectory),
              rootIsDirectory.boolValue else { return false }
        return files.allSatisfy { relativePath in
            var isDirectory: ObjCBool = false
            return fm.fileExists(
                atPath: root.appendingPathComponent(relativePath).path,
                isDirectory: &isDirectory
            ) && !isDirectory.boolValue
        }
    }

    private static let kokoroMaleVoiceNumbers: Set<Int> = [
        9, 10, 11, 12, 13, 14, 15, 16, 20, 25, 29, 30, 31, 33, 34,
        35, 37, 41, 45, 50, 52, 53, 54, 55, 56, 57, 58, 61, 62, 63,
        64, 65, 66, 68, 69, 80, 81, 82, 89, 91, 95, 96, 97, 98, 100,
    ]

    /// Kokoro 中文仓库当前提供 001...100 编号音色，以及三个具名音色。
    static let kokoroVoiceFiles: [String] = [
        "voices/af_maple.safetensors",
        "voices/af_sol.safetensors",
        "voices/bf_vale.safetensors",
    ] + (1...100).map { number in
        let prefix = kokoroMaleVoiceNumbers.contains(number) ? "zm" : "zf"
        return "voices/\(prefix)_\(String(format: "%03d", number)).safetensors"
    }

    static let builtIns: [RecommendedModel] = [
        RecommendedModel(
            id: "qwen3-8b-mlx-4bit",
            title: "Qwen3 8B · MLX 4-bit",
            summary: "本地对话 LLM，Apple Silicon Metal 加速，使用 mlx-lm",
            modelType: "llm",
            provider: "mlx-lm",
            repository: "mlx-community/Qwen3-8B-4bit",
            files: [
                "added_tokens.json",
                "config.json",
                "merges.txt",
                "model.safetensors",
                "model.safetensors.index.json",
                "special_tokens_map.json",
                "tokenizer.json",
                "tokenizer_config.json",
                "vocab.json",
            ],
            directoryName: "qwen3-8b-mlx-4bit",
            estimatedSizeBytes: 4_623_782_544
        ),
        RecommendedModel(
            id: "Qwen3-ASR-0.6B-MLX-4bit",
            title: "Qwen3-ASR 0.6B · MLX 4-bit (Runner)",
            summary: "中文优先语音识别，Apple Silicon Metal 加速（daemon Runner 受管环境；ModelScope: aufklarer/…）",
            modelType: "stt",
            provider: "org.macai.qwen3-asr",
            repository: "aufklarer/Qwen3-ASR-0.6B-MLX-4bit",
            files: [
                "config.json",
                "configuration.json",
                "merges.txt",
                "model.safetensors",
                "model.safetensors.index.json",
                "preprocessor_config.json",
                "tokenizer_config.json",
                "vocab.json",
            ],
            directoryName: "Qwen3-ASR-0.6B-MLX-4bit",
            estimatedSizeBytes: 850_000_000
        ),
        RecommendedModel(
            id: "kokoro-82m-zh",
            title: "Kokoro 82M · 中文 MLX (Runner)",
            summary: "轻量中文语音合成，完整内置 103 个音色（daemon Runner 受管环境）",
            modelType: "tts",
            provider: "org.macai.kokoro",
            repository: "1038lab/Kokoro-82M-zh-MLX",
            files: [
                "config.json",
                "model.safetensors",
            ] + kokoroVoiceFiles,
            directoryName: "kokoro-82m-zh",
            estimatedSizeBytes: 380_917_492
        ),
        RecommendedModel(
            id: "qwen3-tts-0.6b-customvoice-4bit",
            title: "Qwen3-TTS 0.6B · CustomVoice 4-bit",
            summary: "自定义音色中文语音合成，4-bit 量化（MLX/Metal 加速）",
            modelType: "tts",
            provider: "qwen3-tts",
            repository: "mlx-community/Qwen3-TTS-12Hz-0.6B-CustomVoice-4bit",
            files: [
                "config.json",
                "generation_config.json",
                "merges.txt",
                "model.safetensors",
                "model.safetensors.index.json",
                "preprocessor_config.json",
                "speech_tokenizer/config.json",
                "speech_tokenizer/configuration.json",
                "speech_tokenizer/model.safetensors",
                "speech_tokenizer/preprocessor_config.json",
                "tokenizer_config.json",
                "vocab.json",
            ],
            directoryName: "qwen3-tts-0.6b-customvoice-4bit",
            estimatedSizeBytes: 1_693_602_151
        ),
        RecommendedModel(
            id: "whisper-large-v3-turbo-q5",
            title: "Whisper Large v3 Turbo · Q5",
            summary: "多语言语音识别，质量与体积均衡，使用 whisper.cpp",
            modelType: "stt",
            provider: "whisper.cpp",
            repository: "ggerganov/whisper.cpp",
            files: ["ggml-large-v3-turbo-q5_0.bin"],
            directoryName: nil,
            estimatedSizeBytes: 574_041_195
        ),
    ]
}
