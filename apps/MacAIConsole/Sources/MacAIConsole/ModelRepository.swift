import Foundation

/// 统一模型仓库：`~/Library/Application Support/MacAIConsole/Models/` 下
/// 按 llm / tts / stt 分文件夹存放模型文件。
/// GUI 每次刷新实时扫描该目录，放入文件即出现在列表中。
struct RepoModel: Identifiable, Hashable {
    let fileName: String
    let modelType: String   // llm / tts / stt
    let provider: String
    let path: String
    let sizeBytes: UInt64
    let ggufMetadata: GGUFMetadata?
    let detectionError: String?

    init(
        fileName: String,
        modelType: String,
        provider: String,
        path: String,
        sizeBytes: UInt64,
        ggufMetadata: GGUFMetadata? = nil,
        detectionError: String? = nil
    ) {
        self.fileName = fileName
        self.modelType = modelType
        self.provider = provider
        self.path = path
        self.sizeBytes = sizeBytes
        self.ggufMetadata = ggufMetadata
        self.detectionError = detectionError
    }

    /// 注册到 daemon 时使用的模型 ID：文件去掉最后扩展名，目录保留完整名称。
    var modelID: String {
        ModelRepository.defaultModelID(forPath: path)
    }

    var id: String { path }
}

enum ModelRepository {
    static let folderNames = ["llm", "stt", "tts"]

    static func defaultModelID(forPath path: String) -> String {
        let url = URL(fileURLWithPath: path)
        var isDirectory: ObjCBool = false
        if FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory),
           isDirectory.boolValue {
            return url.lastPathComponent
        }
        return url.deletingPathExtension().lastPathComponent
    }

    static var baseURL: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("MacAIConsole", isDirectory: true)
            .appendingPathComponent("Models", isDirectory: true)
    }

    static func folder(for type: String) -> URL {
        baseURL.appendingPathComponent(type, isDirectory: true)
    }

    /// 扫描仓库。目录不存在时自动创建三个分类文件夹。
    static func scan() -> [RepoModel] {
        ensureFolders()

        let fm = FileManager.default
        var models: [RepoModel] = []
        for type in folderNames {
            let url = folder(for: type)
            let items = (try? fm.contentsOfDirectory(at: url, includingPropertiesForKeys: [.fileSizeKey, .isDirectoryKey], options: .skipsHiddenFiles)) ?? []
            for item in items.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
                var isDir: ObjCBool = false
                guard fm.fileExists(atPath: item.path, isDirectory: &isDir) else { continue }
                if isDir.boolValue {
                    // 文件夹形态模型：Kokoro TTS、Qwen3-ASR 或 sherpa-onnx。
                    // Provider 由目录结构与配置文件推导，保证从仓库手动注册时
                    // 仍能选择正确后端。
                    guard let provider = directoryProvider(at: item, type: type) else { continue }
                    models.append(RepoModel(
                        fileName: item.lastPathComponent,
                        modelType: type,
                        provider: provider,
                        path: item.path,
                        sizeBytes: directorySize(at: item)
                    ))
                    continue
                }
                let size = (try? item.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(UInt64.init) ?? 0
                // LLM 引擎已迁移 Runner 架构：注册直接指向 org.macai.llama.cpp，
                // daemon 缺省裁决一致（llm → Runner）。
                let provider = type == "llm" ? "org.macai.llama.cpp" : "whisper.cpp"
                var ggufMetadata: GGUFMetadata?
                var detectionError: String?
                if type == "llm" {
                    if item.pathExtension.lowercased() == "gguf" {
                        do {
                            ggufMetadata = try GGUFMetadataReader.read(from: item)
                        } catch {
                            detectionError = error.localizedDescription
                        }
                    } else {
                        detectionError = "LLM 文件必须使用 .gguf 扩展名"
                    }
                }
                models.append(RepoModel(
                    fileName: item.lastPathComponent,
                    modelType: type,
                    provider: provider,
                    path: item.path,
                    sizeBytes: size,
                    ggufMetadata: ggufMetadata,
                    detectionError: detectionError
                ))
            }
        }
        return models
    }

    /// 确保仓库根目录与 llm/stt/tts 子目录存在。只做 mkdir，不枚举文件。
    static func ensureFolders() {
        let fm = FileManager.default
        for type in folderNames {
            try? fm.createDirectory(at: folder(for: type), withIntermediateDirectories: true)
        }
    }

    /// scan 的后台版本：递归枚举与大小统计离开主线程，模型目录大时避免卡住界面。
    static func scanInBackground() async -> [RepoModel] {
        await Task.detached(priority: .userInitiated) {
            scan()
        }.value
    }

    static func inspectGGUFInBackground(at url: URL) async throws -> GGUFMetadata {
        try await Task.detached(priority: .userInitiated) {
            try GGUFMetadataReader.read(from: url)
        }.value
    }

    private static func directoryProvider(at url: URL, type: String) -> String? {
        let fm = FileManager.default
        if type == "stt" {
            let sherpaFiles = [
                "tokens.txt",
                "encoder.int8.onnx",
                "decoder.onnx",
                "joiner.int8.onnx"
            ]
            if sherpaFiles.allSatisfy({ fm.fileExists(atPath: url.appendingPathComponent($0).path) }) {
                return "org.macai.sherpa-onnx"
            }
            // Qwen3-ASR 模型目录：由 Runner provider（org.macai.qwen3-asr）承接。
            // 以 config.json 的 model_type 判别，4bit / 8bit 目录结构均兼容。
            if fm.fileExists(atPath: url.appendingPathComponent("preprocessor_config.json").path),
               let data = try? Data(contentsOf: url.appendingPathComponent("config.json")),
               let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
               json["model_type"] as? String == "qwen3_asr" {
                return "org.macai.qwen3-asr"
            }
            return nil
        }
        guard fm.fileExists(atPath: url.appendingPathComponent("model.safetensors").path) else {
            return nil
        }
        if type == "tts" {
            // Kokoro 模型目录：由 Runner provider（org.macai.kokoro）承接。
            return "org.macai.kokoro"
        }
        if type == "llm" {
            // MLX LLM 目录（config.json + safetensors）；sharded 权重经 index 文件加载。
            // chat.v1 Runner（org.macai.mlx-lm）承接，legacy mlx-lm 已删除。
            guard fm.fileExists(atPath: url.appendingPathComponent("config.json").path) else {
                return nil
            }
            return "org.macai.mlx-lm"
        }
        return nil
    }

    private static func directorySize(at url: URL) -> UInt64 {
        let keys: Set<URLResourceKey> = [.isRegularFileKey, .fileSizeKey]
        guard let enumerator = FileManager.default.enumerator(
            at: url,
            includingPropertiesForKeys: Array(keys),
            options: .skipsHiddenFiles
        ) else { return 0 }
        var total: UInt64 = 0
        for case let fileURL as URL in enumerator {
            guard let values = try? fileURL.resourceValues(forKeys: keys),
                  values.isRegularFile == true,
                  let size = values.fileSize else { continue }
            total += UInt64(size)
        }
        return total
    }

    /// 把外部模型资源拷入对应类型的文件夹。同名资源直接覆盖（视为更新）。
    static func importFile(at sourceURL: URL, type: String) throws -> URL {
        try importResource(at: sourceURL, type: type, destinationName: sourceURL.lastPathComponent)
    }

    /// 导入文件或目录，可为 Core ML 的 .mlmodelc 编译模型指定目标名称。
    static func importResource(at sourceURL: URL, type: String, destinationName: String) throws -> URL {
        let dest = folder(for: type).appendingPathComponent(destinationName)
        let fm = FileManager.default
        try fm.createDirectory(at: folder(for: type), withIntermediateDirectories: true)
        if fm.fileExists(atPath: dest.path) {
            try fm.removeItem(at: dest)
        }
        try fm.copyItem(at: sourceURL, to: dest)
        return dest
    }

    // MARK: - 每模型设置（上下文长度等），存仓库旁的 model-settings.json

    private static var settingsURL: URL {
        baseURL.deletingLastPathComponent().appendingPathComponent("model-settings.json")
    }

    static func contextLength(for modelID: String) -> Int {
        guard let data = try? Data(contentsOf: settingsURL),
              let map = try? JSONDecoder().decode([String: Int].self, from: data) else { return 4096 }
        return map[modelID] ?? 4096
    }

    /// 模型是否有自定义上下文设置（区分默认 4096 与显式设置）。
    static func customContextLength(for modelID: String) -> Int? {
        guard let data = try? Data(contentsOf: settingsURL),
              let map = try? JSONDecoder().decode([String: Int].self, from: data) else { return nil }
        return map[modelID]
    }

    static func setContextLength(_ length: Int, for modelID: String) {
        var map = [String: Int]()
        if let data = try? Data(contentsOf: settingsURL),
           let decoded = try? JSONDecoder().decode([String: Int].self, from: data) {
            map = decoded
        }
        map[modelID] = length
        try? FileManager.default.createDirectory(at: baseURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        if let data = try? JSONEncoder().encode(map) {
            try? data.write(to: settingsURL, options: .atomic)
        }
    }

    /// 删除模型的本地设置记录（删除模型时调用）。
    static func removeContextLength(for modelID: String) {
        guard let data = try? Data(contentsOf: settingsURL),
              var map = try? JSONDecoder().decode([String: Int].self, from: data),
              map[modelID] != nil else { return }
        map.removeValue(forKey: modelID)
        if let data = try? JSONEncoder().encode(map) {
            try? data.write(to: settingsURL, options: .atomic)
        }
    }
}
