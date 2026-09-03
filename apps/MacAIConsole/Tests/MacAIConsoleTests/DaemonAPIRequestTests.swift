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

    func testPullBuildsDirectoryManifestPayload() async throws {
        let api = try makeAPI(status: 200, body: #"{"id":"kokoro-82m-zh","provider":"org.macai.kokoro"}"#)

        let kokoro = try XCTUnwrap(RecommendedModel.builtIns.first { $0.id == "kokoro-82m-zh" })
        _ = try await api.pull(kokoro, autoLoad: true)

        let request = try XCTUnwrap(RecordingURLProtocol.recorded.first)
        XCTAssertEqual(request.url?.path, "/api/models/pull")
        let payload = try XCTUnwrap(request.bodyData).jsonDictionary
        XCTAssertEqual(payload["repo"] as? String, "1038lab/Kokoro-82M-zh-MLX")
        XCTAssertEqual(payload["model_type"] as? String, "tts")
        XCTAssertEqual(payload["id"] as? String, "kokoro-82m-zh")
        XCTAssertEqual(payload["provider"] as? String, "org.macai.kokoro")
        XCTAssertEqual(payload["auto_load"] as? Bool, true)
        XCTAssertEqual(payload["directory"] as? String, "kokoro-82m-zh")
        XCTAssertNil(payload["filename"])

        // 完整音色清单必须随请求下发：3 个具名音色 + 001...100 编号音色。
        let files = try XCTUnwrap(payload["files"] as? [String])
        XCTAssertEqual(files.count, 105)
        XCTAssertEqual(files.first, "config.json")
        XCTAssertTrue(files.contains("voices/zf_001.safetensors"))
        XCTAssertTrue(files.contains("voices/zm_010.safetensors"))
        XCTAssertTrue(files.contains("voices/af_maple.safetensors"))
        // 下载体积展示的口径也在这里固化（handoff §71：清单唯一固化点）。
        XCTAssertEqual(kokoro.estimatedSizeBytes, 380_917_492)
    }

    func testPullBuildsSingleFilePayloadWithoutDirectoryFields() async throws {
        let api = try makeAPI(status: 200, body: #"{"id":"whisper-large-v3-turbo-q5"}"#)

        let whisper = try XCTUnwrap(RecommendedModel.builtIns.first { $0.id == "whisper-large-v3-turbo-q5" })
        _ = try await api.pull(whisper, autoLoad: false)

        let payload = try XCTUnwrap(RecordingURLProtocol.recorded.first?.bodyData).jsonDictionary
        XCTAssertEqual(payload["filename"] as? String, "ggml-large-v3-turbo-q5_0.bin")
        XCTAssertEqual(payload["auto_load"] as? Bool, false)
        XCTAssertNil(payload["directory"])
        XCTAssertNil(payload["files"])
    }

    func testPullBuildsMlxLmPayloadForRecommendedQwen3() async throws {
        let api = try makeAPI(status: 200, body: #"{"id":"qwen3-8b-mlx-4bit","provider":"org.macai.mlx-lm"}"#)

        let qwen3 = try XCTUnwrap(RecommendedModel.builtIns.first { $0.id == "qwen3-8b-mlx-4bit" })
        _ = try await api.pull(qwen3, autoLoad: true)

        let payload = try XCTUnwrap(RecordingURLProtocol.recorded.first?.bodyData).jsonDictionary
        XCTAssertEqual(payload["repo"] as? String, "mlx-community/Qwen3-8B-4bit")
        XCTAssertEqual(payload["model_type"] as? String, "llm")
        XCTAssertEqual(payload["provider"] as? String, "org.macai.mlx-lm")
        XCTAssertEqual(payload["directory"] as? String, "qwen3-8b-mlx-4bit")
        // worker 只需要权重与 tokenizer；README/.gitattributes 不进清单。
        let files = try XCTUnwrap(payload["files"] as? [String])
        XCTAssertEqual(files.count, 9)
        XCTAssertTrue(files.contains("model.safetensors"))
        XCTAssertTrue(files.contains("model.safetensors.index.json"))
        XCTAssertFalse(files.contains("README.md"))
        XCTAssertEqual(qwen3.estimatedSizeBytes, 4_623_782_544)
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
