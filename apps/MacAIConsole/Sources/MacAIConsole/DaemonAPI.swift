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
    var requestedProvider: String?
    var providerSelectionReason: String?
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
        case requestedProvider = "requested_provider"
        case providerSelectionReason = "provider_selection_reason"
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
    var requestedProvider: String? = nil
    var providerSelectionReason: String? = nil
    var modelType: String
    /// 注册时的模型文件路径。改名不影响它——GUI 用它推导原始/默认 ID。
    var path: String?
    var temperature: Double?
    var topP: Double?

    enum CodingKeys: String, CodingKey {
        case id
        case ownedBy = "owned_by"
        case requestedProvider = "requested_provider"
        case providerSelectionReason = "provider_selection_reason"
        // /v1/models 实际返回的字段名是 "type"（OpenAI 兼容格式）。
        case modelType = "type"
        case path
        case temperature
        case topP = "top_p"
    }

    /// 原始 ID：文件去掉最后扩展名，目录保留完整名称。与改名无关。
    var originalID: String? {
        guard let path else { return nil }
        return ModelRepository.defaultModelID(forPath: path)
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

// MARK: Runner 管理面模型

struct RunnerModel: Decodable, Hashable {
    var profile: String
}

struct RunnerEntry: Decodable, Identifiable, Hashable {
    var id: String
    var root: String
    var state: String
    var environmentId: String
    var phase: String
    var models: [RunnerModel]
    var capabilities: [String]?

    enum CodingKeys: String, CodingKey {
        case id, root, state, models, capabilities
        case environmentId = "environment_id"
        case phase
    }
}

struct RunnerInstallResponse: Decodable {
    var environmentId: String
    var phase: String

    enum CodingKeys: String, CodingKey {
        case phase
        case environmentId = "environment_id"
    }
}

struct LocalDetectorMatch: Decodable, Hashable {
    var runner: String
    var adapter: String
    var capability: String
    var detectorID: String
    var reason: String
    enum CodingKeys: String, CodingKey { case runner, adapter, capability, reason; case detectorID = "detector_id" }
}

struct LocalInspection: Decodable, Hashable {
    var path: String
    var canonicalPath: String?
    var sizeBytes: UInt64?
    var status: String
    var matches: [LocalDetectorMatch]
    var diagnostics: [String]
    var routingToken: String?
    var runnerAvailable: Bool?
    enum CodingKeys: String, CodingKey {
        case path, status, matches, diagnostics
        case canonicalPath = "canonical_path"
        case sizeBytes = "size_bytes"
        case routingToken = "routing_token"
        case runnerAvailable = "runner_available"
    }
}

struct InferenceTaskMessage: Decodable, Hashable {
    var role: String
    var content: String
}

struct InferenceTaskSummary: Decodable, Identifiable, Hashable {
    var id: String
    var kind: String
    var status: String
    var model: String
    var provider: String?
    var inputPreview: String
    var startedAtMs: UInt64
    var completedAtMs: UInt64?
    var durationMs: UInt64?
    var audioDurationMs: UInt64?
    var error: String?

    enum CodingKeys: String, CodingKey {
        case id, kind, status, model, provider, error
        case inputPreview = "input_preview"
        case startedAtMs = "started_at_ms"
        case completedAtMs = "completed_at_ms"
        case durationMs = "duration_ms"
        case audioDurationMs = "audio_duration_ms"
    }

    var isRunning: Bool { status == "running" }

    var realTimeFactor: Double? {
        guard kind == "stt", status == "succeeded",
              let durationMs, let audioDurationMs, audioDurationMs > 0
        else { return nil }
        return Double(durationMs) / Double(audioDurationMs)
    }

    var kindTitle: String {
        switch kind {
        case "chat": "对话生成"
        case "stt": "语音识别"
        case "tts": "语音合成"
        default: kind.uppercased()
        }
    }

    var iconType: String {
        kind == "chat" ? "llm" : kind
    }

    var statusTitle: String {
        switch status {
        case "running": "运行中"
        case "succeeded": "成功"
        case "failed": "失败"
        case "cancelled": "已中断"
        default: status
        }
    }
}

struct InferenceTaskRequest: Decodable {
    var messages: [InferenceTaskMessage]?
    var inputText: String?
    var fileName: String?
    var fileSizeBytes: UInt64?
    var audioDurationMs: UInt64?
    var language: String?
    var voice: String?
    var format: String?
    var speed: Double?
    var stream: Bool?
    var temperature: Double?
    var topP: Double?
    var maxTokens: UInt64?

    enum CodingKeys: String, CodingKey {
        case messages
        case inputText = "input_text"
        case fileName = "file_name"
        case fileSizeBytes = "file_size_bytes"
        case audioDurationMs = "audio_duration_ms"
        case language, voice, format, speed, stream, temperature
        case topP = "top_p"
        case maxTokens = "max_tokens"
    }
}

struct InferenceTaskResult: Decodable {
    var outputText: String?
    var language: String?
    var finishReason: String?
    var promptTokens: UInt64?
    var completionTokens: UInt64?
    var totalTokens: UInt64?
    var tokensPerSecond: Double?
    var contentType: String?
    var byteCount: UInt64?

    enum CodingKeys: String, CodingKey {
        case outputText = "output_text"
        case language
        case finishReason = "finish_reason"
        case promptTokens = "prompt_tokens"
        case completionTokens = "completion_tokens"
        case totalTokens = "total_tokens"
        case tokensPerSecond = "tokens_per_second"
        case contentType = "content_type"
        case byteCount = "byte_count"
    }
}

struct InferenceTaskDetail: Decodable, Identifiable {
    var id: String
    var kind: String
    var status: String
    var model: String
    var provider: String?
    var startedAtMs: UInt64
    var completedAtMs: UInt64?
    var durationMs: UInt64?
    var request: InferenceTaskRequest?
    var result: InferenceTaskResult?
    var requestTruncated: Bool
    var resultTruncated: Bool
    var error: String?

    enum CodingKeys: String, CodingKey {
        case id, kind, status, model, provider, request, result, error
        case startedAtMs = "started_at_ms"
        case completedAtMs = "completed_at_ms"
        case durationMs = "duration_ms"
        case requestTruncated = "request_truncated"
        case resultTruncated = "result_truncated"
    }

    var isRunning: Bool { status == "running" }

    var summary: InferenceTaskSummary {
        InferenceTaskSummary(
            id: id,
            kind: kind,
            status: status,
            model: model,
            provider: provider,
            inputPreview: inputPreview,
            startedAtMs: startedAtMs,
            completedAtMs: completedAtMs,
            durationMs: durationMs,
            audioDurationMs: request?.audioDurationMs,
            error: error
        )
    }

    private var inputPreview: String {
        switch kind {
        case "chat":
            return request?.messages?.last(where: { $0.role == "user" })?.content
                ?? request?.messages?.last?.content
                ?? ""
        case "stt": return request?.fileName ?? ""
        case "tts": return request?.inputText ?? ""
        default: return request?.inputText ?? request?.fileName ?? ""
        }
    }
}

struct InferenceTaskList: Decodable {
    var running: [InferenceTaskSummary]
    var completed: [InferenceTaskSummary]
}

struct LoadResponse: Decodable {
    var id: String
    var provider: String?
    var state: String?
}

struct LoggingLevelResponse: Decodable {
    var level: String
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
    static let defaultBaseURL: URL = {
        guard let url = URL(string: "http://127.0.0.1:11435") else {
            fatalError("The hard-coded aiworkd URL is invalid")
        }
        return url
    }()

    let baseURL: URL
    let session: URLSession

    init(
        baseURL: URL = DaemonAPI.defaultBaseURL,
        session: URLSession = .shared
    ) {
        self.baseURL = baseURL
        self.session = session
    }

    private func send(_ method: String, _ path: String, body: Data?, timeout: TimeInterval) async throws -> Data {
        try await send(
            method,
            url: baseURL.appendingPathComponent(path),
            body: body,
            timeout: timeout
        )
    }

    private func send(_ method: String, url: URL, body: Data?, timeout: TimeInterval) async throws -> Data {
        AppLogger.debug("API \(method) \(url.path)")
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = timeout
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = body
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw URLError(.badServerResponse)
        }
        AppLogger.debug("API \(method) \(url.path) → HTTP \(http.statusCode)")
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
        let data = try body.map { try JSONSerialization.data(withJSONObject: $0) }
        return try await send("POST", path, body: data, timeout: timeout)
    }

    private func delete(_ path: String, timeout: TimeInterval = 30) async throws -> Data {
        try await send("DELETE", path, body: nil, timeout: timeout)
    }

    /// 快速探活。返回 false 表示守护进程连不上。
    func isHealthy() async -> Bool {
        var request = URLRequest(url: baseURL.appendingPathComponent("health"))
        request.timeoutInterval = 1.0
        guard let (_, response) = try? await session.data(for: request),
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

    /// GET /api/model-profiles —— daemon 拥有的可下载模型目录。
    func modelProfiles() async throws -> [ModelProfile] {
        struct Wrapper: Decodable { var data: [ModelProfile] }
        return try JSONDecoder().decode(Wrapper.self, from: try await get("api/model-profiles")).data
    }

    func inspectModels(paths: [String]) async throws -> [LocalInspection] {
        struct Wrapper: Decodable { var data: [LocalInspection] }
        return try JSONDecoder().decode(Wrapper.self, from: try await postJSON("api/models/inspect", body: ["paths": paths], timeout: 15)).data
    }

    /// POST /api/models/remote/inspect —— 获取公开远端模型的文件清单。
    func inspectRemoteModel(source: RemoteModelSource, repo: String) async throws -> RemoteModelInspection {
        let data = try await postJSON(
            "api/models/remote/inspect",
            body: ["source": source.rawValue, "repo": repo],
            timeout: 30
        )
        return try JSONDecoder().decode(RemoteModelInspection.self, from: data)
    }

    // MARK: Runner 管理面

    /// GET /api/runners —— 已发现/受信任 Runner + python 环境 phase。
    func runners() async throws -> [RunnerEntry] {
        struct Wrapper: Decodable { var data: [RunnerEntry] }
        return try JSONDecoder().decode(Wrapper.self, from: try await get("api/runners", timeout: 4)).data
    }

    /// POST /api/runners/{id}/install —— 显式安装 python 环境（uv sync 可达约 10 分钟）。
    func installRunner(_ id: String) async throws -> RunnerInstallResponse {
        try JSONDecoder().decode(
            RunnerInstallResponse.self,
            from: try await postJSON("api/runners/\(id)/install", body: [:], timeout: 660)
        )
    }

    /// DELETE /api/runners/{id}/install —— 删除受管环境，之后可重新安装。
    func uninstallRunner(_ id: String) async throws -> RunnerInstallResponse {
        try JSONDecoder().decode(
            RunnerInstallResponse.self,
            from: try await delete("api/runners/\(id)/install", timeout: 30)
        )
    }

    func runnerScriptTemplate(kind: String) async throws -> String {
        struct Wrapper: Decodable { var source: String }
        return try JSONDecoder().decode(
            Wrapper.self,
            from: try await get("api/runner-scripts/template/\(kind)")
        ).source
    }

    func runnerDependencyPresets() async throws -> [RunnerDependencyPreset] {
        struct Wrapper: Decodable { var data: [RunnerDependencyPreset] }
        return try JSONDecoder().decode(
            Wrapper.self,
            from: try await get("api/runner-scripts/dependency-presets")
        ).data
    }

    func normalizeRunnerDependencies(preset: String, input: String) async throws -> [String] {
        struct Wrapper: Decodable { var dependencies: [String] }
        return try JSONDecoder().decode(
            Wrapper.self,
            from: try await postJSON(
                "api/runner-scripts/dependencies",
                body: ["preset": preset, "input": input]
            )
        ).dependencies
    }

    func inspectRunnerScript(source: String) async throws -> RunnerScriptPreview {
        struct Wrapper: Decodable { var runner: RunnerScriptPreview }
        return try JSONDecoder().decode(
            Wrapper.self,
            from: try await postJSON(
                "api/runner-scripts/inspect",
                body: ["source": source],
                timeout: 15
            )
        ).runner
    }

    func createRunnerScript(source: String, expectedDigest: String) async throws -> Bool {
        struct Response: Decodable {
            var restartRequired: Bool

            enum CodingKeys: String, CodingKey {
                case restartRequired = "restart_required"
            }
        }
        return try JSONDecoder().decode(
            Response.self,
            from: try await postJSON(
                "api/runner-scripts",
                body: ["source": source, "expected_digest": expectedDigest],
                timeout: 1_440
            )
        ).restartRequired
    }

    /// POST /api/logging —— 运行时切换 daemon 的 Info / Debug 过滤级别。
    func setLogLevel(_ level: LogLevel) async throws -> LoggingLevelResponse {
        let data = try await postJSON("api/logging", body: ["level": level.rawValue], timeout: 4)
        return try JSONDecoder().decode(LoggingLevelResponse.self, from: data)
    }

    /// GET /api/tasks?completed_limit=N。N 由客户端先收敛到 1...100。
    func tasks(completedLimit: Int = 100) async throws -> InferenceTaskList {
        var components = URLComponents(
            url: baseURL.appendingPathComponent("api/tasks"),
            resolvingAgainstBaseURL: false
        )
        components?.queryItems = [
            URLQueryItem(
                name: "completed_limit",
                value: String(min(max(completedLimit, 1), 100))
            )
        ]
        guard let url = components?.url else { throw URLError(.badURL) }
        return try JSONDecoder().decode(
            InferenceTaskList.self,
            from: try await send("GET", url: url, body: nil, timeout: 4)
        )
    }

    /// GET /api/tasks/{id}。
    func task(id: String) async throws -> InferenceTaskDetail {
        let data = try await get("api/tasks/\(id)")
        return try JSONDecoder().decode(InferenceTaskDetail.self, from: data)
    }

    /// POST /api/models/load —— 注册并加载本地模型；缺省 Runner 选择由 daemon 完成。
    func registerAndLoad(path: String, id: String?, name: String?, contextLength: Int?, keepAlive: String?, modelType: String? = nil, provider: String? = nil, routingToken: String? = nil) async throws -> LoadResponse {
        var body: [String: Any] = ["path": path]
        if let id { body["id"] = id }
        if let name { body["name"] = name }
        if let modelType { body["model_type"] = modelType }
        if let provider { body["provider"] = provider }
        if let routingToken { body["routing_token"] = routingToken }
        if let contextLength { body["context_length"] = contextLength }
        if let keepAlive { body["keep_alive"] = keepAlive }
        let data = try await postJSON("api/models/load", body: body, timeout: 180)
        return try JSONDecoder().decode(LoadResponse.self, from: data)
    }

    /// POST /api/models/{id}/generation —— 更新 LLM 每模型默认采样参数。
    func setGenerationSettings(_ id: String, temperature: Double, topP: Double) async throws {
        _ = try await postJSON(
            "api/models/\(id)/generation",
            body: ["temperature": temperature, "top_p": topP]
        )
    }

    /// POST /api/model-profiles/{id}/pull —— daemon 根据 Profile 下载并可选加载。
    func pullProfile(_ id: String, autoLoad: Bool) async throws -> LoadResponse {
        let data = try await postJSON(
            "api/model-profiles/\(id)/pull",
            body: ["auto_load": autoLoad],
            timeout: 7_200
        )
        return try JSONDecoder().decode(LoadResponse.self, from: data)
    }

    /// POST /api/models/pull —— 下载用户从远端清单中选择的文件，不注册或加载。
    func pullRemoteModel(
        _ inspection: RemoteModelInspection,
        modelType: String,
        files: [RemoteModelFile],
        singleFile: Bool
    ) async throws -> LoadResponse {
        var body: [String: Any] = [
            "source": inspection.source.rawValue,
            "repo": inspection.repo,
            "revision": inspection.revision,
            "model_type": modelType,
            "auto_load": false,
        ]
        if singleFile, let file = files.first {
            body["filename"] = file.path
        } else {
            body["directory"] = inspection.directory
            body["files"] = files.map(\.path)
        }
        let data = try await postJSON("api/models/pull", body: body, timeout: 7_200)
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
