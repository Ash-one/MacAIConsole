import Foundation

/// 统一模型仓库：`~/Library/Application Support/MacAIConsole/Models/` 下
/// 按 llm / tts / stt 分文件夹存放模型文件。
/// GUI 枚举模型仓库顶层项目；目录能力与 Runner 路由由 daemon inspect API 决定。
struct RepoModel: Identifiable, Hashable {
    let fileName: String
    let modelType: String   // llm / tts / stt
    let path: String
    let sizeBytes: UInt64
    let isDirectory: Bool
    let ggufMetadata: GGUFMetadata?
    let detectionError: String?

    init(
        fileName: String,
        modelType: String,
        path: String,
        sizeBytes: UInt64,
        isDirectory: Bool = false,
        ggufMetadata: GGUFMetadata? = nil,
        detectionError: String? = nil
    ) {
        self.fileName = fileName
        self.modelType = modelType
        self.path = path
        self.sizeBytes = sizeBytes
        self.isDirectory = isDirectory
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
                    models.append(RepoModel(fileName: item.lastPathComponent, modelType: type, path: item.path, sizeBytes: 0, isDirectory: true))
                    continue
                }
                let ext = item.pathExtension.lowercased()
                guard (type == "llm" && ext == "gguf") || (type == "stt" && ext == "bin") else { continue }
                let size = (try? item.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(UInt64.init) ?? 0
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
