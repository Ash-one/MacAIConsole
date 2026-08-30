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
        let fm = FileManager.default
        try? fm.createDirectory(at: folder(for: "llm"), withIntermediateDirectories: true)
        try? fm.createDirectory(at: folder(for: "tts"), withIntermediateDirectories: true)
        try? fm.createDirectory(at: folder(for: "stt"), withIntermediateDirectories: true)

        var models: [RepoModel] = []
        for type in folderNames {
            let url = folder(for: type)
            let items = (try? fm.contentsOfDirectory(at: url, includingPropertiesForKeys: [.fileSizeKey, .isDirectoryKey], options: .skipsHiddenFiles)) ?? []
            for item in items.sorted(by: { $0.lastPathComponent < $1.lastPathComponent }) {
                var isDir: ObjCBool = false
                guard fm.fileExists(atPath: item.path, isDirectory: &isDir) else { continue }
                if isDir.boolValue {
                    // 文件夹形态模型：Kokoro TTS 或 Qwen3-ASR。Provider 由目录结构与
                    // Qwen 量化配置推导，保证从仓库手动注册时仍能选择正确后端。
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
                let provider = type == "llm" ? "llama.cpp" : "whisper.cpp"
                models.append(RepoModel(fileName: item.lastPathComponent, modelType: type, provider: provider, path: item.path, sizeBytes: size))
            }
        }
        return models
    }

    private static func directoryProvider(at url: URL, type: String) -> String? {
        let fm = FileManager.default
        guard fm.fileExists(atPath: url.appendingPathComponent("model.safetensors").path) else {
            return nil
        }
        if type == "tts" { return "kokoro-mlx" }
        guard type == "stt",
              fm.fileExists(atPath: url.appendingPathComponent("preprocessor_config.json").path),
              let data = try? Data(contentsOf: url.appendingPathComponent("config.json")),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        let quantization = (json["quantization"] as? [String: Any])
            ?? (json["quantization_config"] as? [String: Any])
        return (quantization?["bits"] as? NSNumber)?.intValue == 8
            ? "qwen3-asr-mlx"
            : "qwen3-asr"
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
