import Foundation

/// 统一模型仓库：`~/Library/Application Support/MacAIConsole/Models/` 下
/// 按 llm / tts / stt 分文件夹存放模型文件。
/// GUI 每次刷新实时扫描该目录，放入文件即出现在列表中。
struct RepoModel: Identifiable, Hashable {
    let fileName: String
    let modelType: String   // llm / tts / stt
    let path: String
    let sizeBytes: UInt64

    /// 注册到 daemon 时使用的模型 ID：去掉扩展名的文件名。
    var modelID: String {
        (fileName as NSString).deletingPathExtension
    }

    var id: String { path }
}

enum ModelRepository {
    static let folderNames = ["llm", "stt", "tts"]

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
                    // 文件夹形态模型（如 Kokoro：model.safetensors + voices/）。
                    // 目前仅 TTS 使用；目录内需有 model.safetensors 才视为有效模型。
                    guard type == "tts",
                          fm.fileExists(atPath: item.appendingPathComponent("model.safetensors").path) else { continue }
                    models.append(RepoModel(fileName: item.lastPathComponent, modelType: type, path: item.path, sizeBytes: 0))
                    continue
                }
                let size = (try? item.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(UInt64.init) ?? 0
                models.append(RepoModel(fileName: item.lastPathComponent, modelType: type, path: item.path, sizeBytes: size))
            }
        }
        return models
    }

    /// 把外部文件拷入对应类型的文件夹。同名文件直接覆盖（视为更新）。
    static func importFile(at sourceURL: URL, type: String) throws -> URL {
        let dest = folder(for: type).appendingPathComponent(sourceURL.lastPathComponent)
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
