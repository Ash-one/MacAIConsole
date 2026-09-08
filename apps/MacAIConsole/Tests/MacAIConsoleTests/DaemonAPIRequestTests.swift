#if canImport(XCTest)
import Foundation
import XCTest
@testable import MacAIConsole

/// daemon 端 `validate_pull_request` 只接受两种互斥形态（单文件 / directory+files），
/// GUI 构造的请求体是这份契约的另一半，这里逐字段固化。
final class DaemonAPIRequestTests: XCTestCase {
    override func setUp() {
        super.setUp()
        RecordingURLProtocol.reset()
    }

    func testModelProfileCatalogDecodesUnknownRunnerWithoutSwiftCatalog() async throws {
        let api = try makeAPI(status: 200, body: #"{"data":[{"id":"future","name":"Future Model","model_type":"stt","runner":"org.example.future","source_repo":"owner/model","directory":"future","files":["model.bin"],"memory_estimate_bytes":123}]}"#)

        let profiles = try await api.modelProfiles()
        let profile = try XCTUnwrap(profiles.first)
        XCTAssertEqual(profile.id, "future")
        XCTAssertEqual(profile.runner, "org.example.future")
        XCTAssertEqual(profile.modelType, "stt")
        XCTAssertEqual(profile.files, ["model.bin"])
        XCTAssertEqual(RecordingURLProtocol.recorded.first?.url?.path, "/api/model-profiles")
    }

    func testProfilePullDoesNotDuplicateProfileFieldsInGUIRequest() async throws {
        let api = try makeAPI(status: 200, body: #"{"id":"future","state":"downloaded"}"#)
        _ = try await api.pullProfile("future", autoLoad: false)

        let request = try XCTUnwrap(RecordingURLProtocol.recorded.first)
        XCTAssertEqual(request.url?.path, "/api/model-profiles/future/pull")
        let payload = try XCTUnwrap(request.bodyData).jsonDictionary
        XCTAssertEqual(payload["auto_load"] as? Bool, false)
        XCTAssertEqual(payload.count, 1)
    }

    func testRemoteInspectAndPullUsePinnedRevisionWithoutAutoLoad() async throws {
        let response = #"{"source":"modelscope","repo":"owner/model","revision":"abc123","directory":"owner--model","files":[{"path":"config.json","size_bytes":12},{"path":"weights/model.safetensors","size_bytes":34}]}"#
        let api = try makeAPI(status: 200, body: response)
        let inspection = try await api.inspectRemoteModel(source: .modelscope, repo: "owner/model")

        let inspectPayload = try XCTUnwrap(RecordingURLProtocol.recorded.last?.bodyData).jsonDictionary
        XCTAssertEqual(inspectPayload["source"] as? String, "modelscope")
        XCTAssertEqual(inspectPayload["repo"] as? String, "owner/model")

        RecordingURLProtocol.reset()
        RecordingURLProtocol.enqueue(status: 200, body: #"{"id":"owner--model","state":"downloaded"}"#)
        _ = try await api.pullRemoteModel(
            inspection,
            modelType: "llm",
            files: inspection.files,
            singleFile: false
        )
        let pullPayload = try XCTUnwrap(RecordingURLProtocol.recorded.last?.bodyData).jsonDictionary
        XCTAssertEqual(pullPayload["source"] as? String, "modelscope")
        XCTAssertEqual(pullPayload["revision"] as? String, "abc123")
        XCTAssertEqual(pullPayload["directory"] as? String, "owner--model")
        XCTAssertEqual(pullPayload["files"] as? [String], ["config.json", "weights/model.safetensors"])
        XCTAssertEqual(pullPayload["auto_load"] as? Bool, false)
        XCTAssertNil(pullPayload["filename"])
    }

    func testRemoteSingleFilePullUsesFilenameShape() async throws {
        let response = #"{"source":"huggingface","repo":"owner/model","revision":"abc123","directory":"owner--model","files":[{"path":"weights/model-q4.gguf","size_bytes":12}]}"#
        let api = try makeAPI(status: 200, body: response)
        let inspection = try await api.inspectRemoteModel(source: .huggingface, repo: "owner/model")

        RecordingURLProtocol.reset()
        RecordingURLProtocol.enqueue(status: 200, body: #"{"id":"model-q4","state":"downloaded"}"#)
        _ = try await api.pullRemoteModel(
            inspection,
            modelType: "llm",
            files: inspection.files,
            singleFile: true
        )
        let payload = try XCTUnwrap(RecordingURLProtocol.recorded.last?.bodyData).jsonDictionary
        XCTAssertEqual(payload["filename"] as? String, "weights/model-q4.gguf")
        XCTAssertNil(payload["files"])
        XCTAssertNil(payload["directory"])
        XCTAssertEqual(payload["auto_load"] as? Bool, false)
    }

    func testRegisterAndLoadOmitsUnspecifiedFields() async throws {
        let api = try makeAPI(status: 200, body: #"{"id":"m","provider":"llama.cpp"}"#)

        _ = try await api.registerAndLoad(path: "/Models/llm/m.gguf", id: "m", name: nil, contextLength: 8192, keepAlive: "30m", modelType: "llm", provider: "llama.cpp")
        let full = try XCTUnwrap(RecordingURLProtocol.recorded.last?.bodyData).jsonDictionary
        XCTAssertEqual(full["path"] as? String, "/Models/llm/m.gguf")
        XCTAssertEqual(full["context_length"] as? Int, 8192)
        XCTAssertEqual(full["keep_alive"] as? String, "30m")
        XCTAssertEqual(full["model_type"] as? String, "llm")
        XCTAssertNil(full["name"])

        RecordingURLProtocol.reset()
        RecordingURLProtocol.enqueue(status: 200, body: #"{"id":"m"}"#)
        _ = try await api.registerAndLoad(path: "/Models/tts/kokoro", id: nil, name: nil, contextLength: nil, keepAlive: nil)
        let minimal = try XCTUnwrap(RecordingURLProtocol.recorded.last?.bodyData).jsonDictionary
        XCTAssertEqual(minimal["path"] as? String, "/Models/tts/kokoro")
        XCTAssertEqual(minimal.count, 1)
    }

    func testDirectoryRoutingUsesDaemonTokenWithoutDuplicatingProviderRules() async throws {
        let api = try makeAPI(status: 200, body: #"{"id":"qwen","provider":"org.macai.qwen3-asr"}"#)
        _ = try await api.registerAndLoad(
            path: "/Models/stt/qwen",
            id: "qwen",
            name: nil,
            contextLength: nil,
            keepAlive: nil,
            routingToken: "short-lived-token"
        )

        let payload = try XCTUnwrap(RecordingURLProtocol.recorded.last?.bodyData).jsonDictionary
        XCTAssertEqual(payload["routing_token"] as? String, "short-lived-token")
        XCTAssertNil(payload["provider"])
        XCTAssertNil(payload["model_type"])
    }

    func testServerErrorMessageOverridesGenericFallback() async throws {
        let api = try makeAPI(
            status: 500,
            body: #"{"error":{"type":"model_load_failed","message":"worker 进程启动失败"}}"#
        )
        do {
            _ = try await api.loadRegistered("m")
            XCTFail("expected error")
        } catch let error as DaemonError {
            XCTAssertEqual(error.errorDescription, "HTTP 500：worker 进程启动失败")
        }

        RecordingURLProtocol.reset()
        let opaqueAPI = try makeAPI(status: 502, body: "bad gateway")
        do {
            _ = try await opaqueAPI.loadRegistered("m")
            XCTFail("expected error")
        } catch let error as DaemonError {
            // 非 JSON 错误体退回通用文案，状态码必须保留。
            XCTAssertEqual(error.errorDescription, "HTTP 502：请求失败")
        }
    }

    func testTasksCompletedLimitIsClampedInto1to100() async throws {
        let api = try makeAPI(status: 200, body: #"{"running":[],"completed":[]}"#)
        RecordingURLProtocol.enqueue(status: 200, body: #"{"running":[],"completed":[]}"#)

        _ = try await api.tasks(completedLimit: 0)
        _ = try await api.tasks(completedLimit: 1000)

        let limits = RecordingURLProtocol.recorded.compactMap { request -> Int? in
            guard let query = request.url?.query,
                  let components = URLComponents(string: "?\(query)") else { return nil }
            return components.queryItems?.first { $0.name == "completed_limit" }?.value.flatMap(Int.init)
        }
        XCTAssertEqual(limits, [1, 100])
    }

    func testRunnersDecodesEngineEntriesAndCapabilities() async throws {
        let json = """
        {
            "data": [
                {
                    "id": "org.macai.llama-cpp",
                    "root": "/runners/llama.cpp",
                    "state": "trusted",
                    "environment_id": "llama-cpp",
                    "phase": "ready",
                    "capabilities": ["chat", "completion"],
                    "models": []
                }
            ]
        }
        """
        let api = try makeAPI(status: 200, body: json)
        let runners = try await api.runners()
        XCTAssertEqual(runners.count, 1)
        let first = try XCTUnwrap(runners.first)
        XCTAssertEqual(first.id, "org.macai.llama-cpp")
        XCTAssertEqual(first.phase, "ready")
        XCTAssertEqual(first.capabilities, ["chat", "completion"])
    }

    func testInstallRunnerSendsPostToRunnersInstallEndpoint() async throws {
        let api = try makeAPI(status: 200, body: #"{"environment_id":"llama-cpp","phase":"ready"}"#)
        let res = try await api.installRunner("org.macai.llama-cpp")
        XCTAssertEqual(res.phase, "ready")
        XCTAssertEqual(RecordingURLProtocol.recorded.first?.url?.path, "/api/runners/org.macai.llama-cpp/install")
        XCTAssertEqual(RecordingURLProtocol.recorded.first?.httpMethod, "POST")
    }

    private func makeAPI(status: Int, body: String) throws -> DaemonAPI {
        RecordingURLProtocol.enqueue(status: status, body: body)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [RecordingURLProtocol.self]
        let baseURL = try XCTUnwrap(URL(string: "http://127.0.0.1:11435"))
        return DaemonAPI(
            baseURL: baseURL,
            session: URLSession(configuration: configuration)
        )
    }
}

private final class RecordingURLProtocol: URLProtocol {
    private static let lock = NSLock()
    private static var responses: [(status: Int, body: Data)] = []
    private static var requests: [URLRequest] = []

    static var recorded: [URLRequest] {
        lock.lock()
        defer { lock.unlock() }
        return requests
    }

    static func reset() {
        lock.lock()
        defer { lock.unlock() }
        responses = []
        requests = []
    }

    static func enqueue(status: Int, body: String) {
        lock.lock()
        defer { lock.unlock() }
        responses.append((status, Data(body.utf8)))
    }

    override class func canInit(with request: URLRequest) -> Bool { true }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.lock.lock()
        Self.requests.append(request)
        let response = Self.responses.isEmpty
            ? (status: 200, body: Data("{}".utf8))
            : Self.responses.removeFirst()
        Self.lock.unlock()

        guard let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let http = HTTPURLResponse(
            url: url,
            statusCode: response.status,
            httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        )
        client?.urlProtocol(self, didReceive: http!, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: response.body)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

private extension URLRequest {
    /// URLSession 会把 httpBody 转成流式 body，URLProtocol 侧只能从 stream 读取。
    var bodyData: Data? {
        if let httpBody { return httpBody }
        guard let stream = httpBodyStream else { return nil }
        stream.open()
        defer { stream.close() }
        var data = Data()
        let bufferSize = 4096
        let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: bufferSize)
        defer { buffer.deallocate() }
        while stream.hasBytesAvailable {
            let read = stream.read(buffer, maxLength: bufferSize)
            if read <= 0 { break }
            data.append(buffer, count: read)
        }
        return data
    }
}

private extension Data {
    var jsonDictionary: [String: Any] {
        get throws {
            try XCTUnwrap(
                JSONSerialization.jsonObject(with: self) as? [String: Any],
                "request body must be a JSON object"
            )
        }
    }
}
#endif
