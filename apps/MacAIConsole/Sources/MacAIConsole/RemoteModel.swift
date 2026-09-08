import Foundation

enum RemoteModelSource: String, Codable, CaseIterable, Identifiable {
    case huggingface
    case modelscope

    var id: String { rawValue }
    var title: String {
        switch self {
        case .huggingface: "Hugging Face"
        case .modelscope: "ModelScope"
        }
    }
}

struct RemoteModelFile: Codable, Identifiable, Hashable {
    let path: String
    let sizeBytes: UInt64

    var id: String { path }

    enum CodingKeys: String, CodingKey {
        case path
        case sizeBytes = "size_bytes"
    }
}

struct RemoteModelInspection: Decodable {
    let source: RemoteModelSource
    let repo: String
    let revision: String
    let directory: String
    let files: [RemoteModelFile]
}

struct RemoteModelReference: Equatable {
    let source: RemoteModelSource
    let repo: String

    static func parse(_ input: String, selectedSource: RemoteModelSource) throws -> Self {
        let value = input.trimmingCharacters(in: .whitespacesAndNewlines)
        if let url = URL(string: value), let scheme = url.scheme, !scheme.isEmpty {
            guard scheme == "https", let host = url.host?.lowercased() else {
                throw ValidationError.invalidURL
            }
            let parts = url.pathComponents.filter { $0 != "/" }
            if host == "huggingface.co" || host == "www.huggingface.co" {
                guard parts.count == 2 else { throw ValidationError.invalidURL }
                return try validated(source: .huggingface, repo: parts.joined(separator: "/"))
            }
            if host == "modelscope.cn" || host == "www.modelscope.cn" {
                guard parts.count == 3, parts[0] == "models" else {
                    throw ValidationError.invalidURL
                }
                return try validated(source: .modelscope, repo: parts.dropFirst().joined(separator: "/"))
            }
            throw ValidationError.invalidURL
        }
        return try validated(source: selectedSource, repo: value)
    }

    private static func validated(source: RemoteModelSource, repo: String) throws -> Self {
        let parts = repo.split(separator: "/", omittingEmptySubsequences: false)
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "._-"))
        guard parts.count == 2,
              parts.allSatisfy({ !$0.isEmpty && $0.unicodeScalars.allSatisfy(allowed.contains) }) else {
            throw ValidationError.invalidRepo
        }
        return Self(source: source, repo: repo)
    }

    enum ValidationError: LocalizedError {
        case invalidRepo
        case invalidURL

        var errorDescription: String? {
            switch self {
            case .invalidRepo: "模型 ID 必须使用 owner/repo 格式"
            case .invalidURL: "只支持 Hugging Face 或 ModelScope 的 HTTPS 模型页面"
            }
        }
    }
}

enum RemoteModelSelection {
    static func singleFileCandidates(modelType: String, files: [RemoteModelFile]) -> [RemoteModelFile] {
        switch modelType {
        case "llm": files.filter { $0.path.lowercased().hasSuffix(".gguf") }
        case "stt": files.filter {
            let name = URL(fileURLWithPath: $0.path).lastPathComponent.lowercased()
            return name.hasPrefix("ggml-") && name.hasSuffix(".bin")
        }
        default: []
        }
    }
}
