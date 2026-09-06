import Foundation

/// daemon Model Profile catalog 中供 GUI 展示与触发下载的只读描述。
struct ModelProfile: Decodable, Identifiable, Hashable {
    let id: String
    let name: String
    let modelType: String
    let runner: String
    let sourceRepo: String
    let directory: String?
    let files: [String]
    let memoryEstimateBytes: UInt64?

    enum CodingKeys: String, CodingKey {
        case id, name, runner, directory, files
        case modelType = "model_type"
        case sourceRepo = "source_repo"
        case memoryEstimateBytes = "memory_estimate_bytes"
    }

    var repositoryURL: URL? { URL(string: "https://huggingface.co/\(sourceRepo)") }

    var localURL: URL {
        let folder = ModelRepository.folder(for: modelType)
        if let directory { return folder.appendingPathComponent(directory, isDirectory: true) }
        return folder.appendingPathComponent(files.first ?? id)
    }

    var downloadedURL: URL? {
        let fm = FileManager.default
        let url = localURL
        var isDirectory: ObjCBool = false
        guard fm.fileExists(atPath: url.path, isDirectory: &isDirectory) else { return nil }
        if directory != nil {
            guard isDirectory.boolValue else { return nil }
            return files.allSatisfy { fm.fileExists(atPath: url.appendingPathComponent($0).path) } ? url : nil
        }
        return isDirectory.boolValue ? nil : url
    }

    var isDownloaded: Bool { downloadedURL != nil }
}
