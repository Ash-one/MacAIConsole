import Foundation

// MARK: - 数据模型（对齐 aiworkd 管理 API）

struct HealthResponse: Decodable {
    let status: String
    let version: String
    let modelCount: Int

    enum CodingKeys: String, CodingKey {
        case status, version
        case modelCount = "model_count"
    }
}

struct RuntimeInfo: Decodable {
    var version: String
    var pid: UInt32
    var uptimeSecs: UInt64
    var activeRequests: UInt64
    var loadedModels: [LoadedModel]
    /// AI 内存预算（字节）；daemon 无法探测时缺失。
    var memoryBudget: UInt64?
    /// 物理内存总量（字节），供内存压力条展示。
    var memoryTotal: UInt64?
    /// 当前已使用物理内存（字节）。
    var memoryUsed: UInt64?

    enum CodingKeys: String, CodingKey {
        case version, pid
        case uptimeSecs = "uptime_secs"
        case activeRequests = "active_requests"
        case loadedModels = "loaded_models"
        case memoryBudget = "memory_budget"
        case memoryTotal = "memory_total"
        case memoryUsed = "memory_used"
    }
}

struct LoadedModel: Decodable, Identifiable, Hashable {
    var id: String
    var provider: String
    var state: String
    var memoryEstimate: UInt64?
    var memoryUsageBytes: UInt64?
    var keepAlive: String?
    var loadedAt: UInt64?
    var lastUsedAt: UInt64?
    var contextLength: Int?
    var modelType: String?
    var defaultVoice: String?
    /// 当前生效的加速设备（coreml / metal / gpu / cpu）；未探测到时为空。
    var effectiveDevice: String?

    enum CodingKeys: String, CodingKey {
        case id, provider, state
        case memoryEstimate = "memory_estimate"
        case memoryUsageBytes = "memory_usage_bytes"
        case keepAlive = "keep_alive"
        case loadedAt = "loaded_at"
        case lastUsedAt = "last_used_at"
        case contextLength = "context_length"
        case modelType = "model_type"
        case defaultVoice = "default_voice"
        case effectiveDevice = "effective_device"
    }
}

struct VoiceResponse: Decodable {
    var voices: [String]
    var defaultVoice: String?

    enum CodingKeys: String, CodingKey {
        case voices
        case defaultVoice = "default_voice"
    }
}

struct ModelEntry: Decodable, Identifiable, Hashable {
    var id: String
    var ownedBy: String
    var modelType: String
    /// 注册时的模型文件路径。改名不影响它——GUI 用它推导原始/默认 ID。
    var path: String?

    enum CodingKeys: String, CodingKey {
        case id
        case ownedBy = "owned_by"
        // /v1/models 实际返回的字段名是 "type"（OpenAI 兼容格式）。
        case modelType = "type"
        case path
    }

    /// 原始 ID = 注册来源文件名（去扩展名）。与改名无关，始终可恢复。
    var originalID: String? {
        guard let path else { return nil }
        let name = (path as NSString).lastPathComponent
        return (name as NSString).deletingPathExtension
    }
}

struct ProviderDescriptor: Decodable {
    var id: String
    var capabilities: [String]?
    var isolation: String?
    var supportedDevices: [String]?

    enum CodingKeys: String, CodingKey {
        case id, capabilities, isolation
        case supportedDevices = "supported_devices"
    }
}

struct ProviderStatus: Decodable {
    var available: Bool
    var ready: Bool
    var effectiveDevice: String?
    var residentModels: [String]?
    var reason: String?
    var installHint: String?

    enum CodingKeys: String, CodingKey {
        case available, ready, reason
        case effectiveDevice = "effective_device"
        case residentModels = "resident_models"
        case installHint = "install_hint"
    }
}

struct ProviderEntry: Decodable, Identifiable {
    var descriptor: ProviderDescriptor
    var status: ProviderStatus

    var id: String { descriptor.id }
}

struct LoadResponse: Decodable {
    var id: String
    var provider: String?
    var state: String?
}

struct APIErrorBody: Decodable {
    struct Inner: Decodable {
        var type: String
        var message: String
    }

    var error: Inner
}

enum DaemonError: LocalizedError {
    case http(status: Int, message: String)

    var errorDescription: String? {
        switch self {
        case let .http(status, message):
            return "HTTP \(status)：\(message)"
        }
    }
}

// MARK: - 客户端

struct DaemonAPI {
    var baseURL: URL

    init(baseURL: URL = URL(string: "http://127.0.0.1:11435")!) {
        self.baseURL = baseURL
    }

    private func send(_ method: String, _ path: String, body: Data?, timeout: TimeInterval) async throws -> Data {
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = method
        request.timeoutInterval = timeout
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = body
        }
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw URLError(.badServerResponse)
        }
        guard (200..<300).contains(http.statusCode) else {
            if let detail = try? JSONDecoder().decode(APIErrorBody.self, from: data) {
                throw DaemonError.http(status: http.statusCode, message: detail.error.message)
            }
            throw DaemonError.http(status: http.statusCode, message: "请求失败")
        }
        return data
    }

    private func get(_ path: String, timeout: TimeInterval = 4) async throws -> Data {
        try await send("GET", path, body: nil, timeout: timeout)
    }

    private func postJSON(_ path: String, body: [String: Any]?, timeout: TimeInterval = 30) async throws -> Data {
        let data = body.map { try! JSONSerialization.data(withJSONObject: $0) }
        return try await send("POST", path, body: data, timeout: timeout)
    }

    /// 快速探活。返回 false 表示守护进程连不上。
    func isHealthy() async -> Bool {
        var request = URLRequest(url: baseURL.appendingPathComponent("health"))
        request.timeoutInterval = 1.0
        guard let (_, response) = try? await URLSession.shared.data(for: request),
              let http = response as? HTTPURLResponse else { return false }
        return http.statusCode == 200
    }

    /// GET /api/runtime
    func runtimeInfo() async throws -> RuntimeInfo {
        try JSONDecoder().decode(RuntimeInfo.self, from: try await get("api/runtime"))
    }

    /// GET /v1/models
    func models() async throws -> [ModelEntry] {
        struct Wrapper: Decodable { var data: [ModelEntry] }
        return try JSONDecoder().decode(Wrapper.self, from: try await get("v1/models")).data
    }

    /// GET /api/providers
    func providers() async throws -> [ProviderEntry] {
        struct Wrapper: Decodable { var data: [ProviderEntry] }
        return try JSONDecoder().decode(Wrapper.self, from: try await get("api/providers")).data
    }

    /// POST /api/models/load —— 注册并加载模型（llm→llama.cpp/.gguf，stt→whisper.cpp/.bin）
    func registerAndLoad(path: String, id: String?, name: String?, contextLength: Int?, keepAlive: String?, modelType: String? = nil) async throws -> LoadResponse {
        var body: [String: Any] = ["path": path]
        if let id { body["id"] = id }
        if let name { body["name"] = name }
        if let modelType { body["model_type"] = modelType }
        if let contextLength { body["context_length"] = contextLength }
        if let keepAlive { body["keep_alive"] = keepAlive }
        let data = try await postJSON("api/models/load", body: body, timeout: 180)
        return try JSONDecoder().decode(LoadResponse.self, from: data)
    }

    /// POST /api/models/{id}/load
    func loadRegistered(_ id: String) async throws -> LoadResponse {
        let data = try await postJSON("api/models/\(id)/load", body: nil, timeout: 180)
        return try JSONDecoder().decode(LoadResponse.self, from: data)
    }

    /// POST /api/models/{id}/unload
    func unload(_ id: String) async throws -> LoadResponse {
        let data = try await postJSON("api/models/\(id)/unload", body: nil, timeout: 30)
        return try JSONDecoder().decode(LoadResponse.self, from: data)
    }

    /// DELETE /api/models/{id} —— 从注册表删除模型（已加载时先卸载 worker）。
    func unregister(_ id: String) async throws {
        _ = try await send("DELETE", "api/models/\(id)", body: nil, timeout: 30)
    }

    /// POST /api/models/{id}/rename —— 重命名模型 ID（设置保留，已加载先卸载）。
    func rename(_ id: String, to newID: String) async throws {
        _ = try await postJSON("api/models/\(id)/rename", body: ["new_id": newID], timeout: 30)
    }

    /// POST /api/models/{id}/keep-alive —— 只改策略，进程保持常驻。
    func setKeepAlive(_ id: String, keepAlive: String?) async throws {
        var body: [String: Any] = [:]
        if let keepAlive { body["keep_alive"] = keepAlive }
        _ = try await postJSON("api/models/\(id)/keep-alive", body: body, timeout: 10)
    }

    /// POST /api/models/{id}/voice —— 修改 TTS 模型默认音色。
    func setVoice(_ id: String, voice: String) async throws {
        _ = try await postJSON("api/models/\(id)/voice", body: ["voice": voice], timeout: 10)
    }

    /// GET /api/models/{id}/voices —— 获取 TTS 模型可用音色。
    func voices(_ id: String) async throws -> VoiceResponse {
        try JSONDecoder().decode(VoiceResponse.self, from: try await get("api/models/\(id)/voices"))
    }

    /// POST /v1/audio/speech —— 返回 WAV 音频数据（试听用）。
    func synthesizeSpeech(modelID: String, text: String, voice: String) async throws -> Data {
        var request = URLRequest(url: baseURL.appendingPathComponent("v1/audio/speech"))
        request.httpMethod = "POST"
        request.timeoutInterval = 180
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body: [String: Any] = [
            "model": modelID,
            "input": text,
            "voice": voice,
            "response_format": "wav",
        ]
        request.httpBody = try JSONSerialization.data(withJSONObject: body)
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw URLError(.badServerResponse)
        }
        guard (200..<300).contains(http.statusCode) else {
            if let detail = try? JSONDecoder().decode(APIErrorBody.self, from: data) {
                throw DaemonError.http(status: http.statusCode, message: detail.error.message)
            }
            throw DaemonError.http(status: http.statusCode, message: "合成失败")
        }
        return data
    }
}