#if canImport(XCTest)
import Foundation
import XCTest
@testable import MacAIConsole

final class RecommendationPullTests: XCTestCase {
    override func tearDown() {
        RecommendationURLProtocol.requestBody = nil
        super.tearDown()
    }

    func testDirectoryRecommendationSendsProviderAndCompleteManifest() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [RecommendationURLProtocol.self]
        let session = URLSession(configuration: configuration)
        let api = DaemonAPI(
            baseURL: try XCTUnwrap(URL(string: "http://127.0.0.1:11435")),
            session: session
        )
        let model = try XCTUnwrap(
            RecommendedModel.builtIns.first { $0.provider == "qwen3-asr-mlx" }
        )

        let response = try await api.pull(model, autoLoad: true)

        XCTAssertEqual(response.id, model.id)
        let body = try XCTUnwrap(RecommendationURLProtocol.requestBody)
        let json = try XCTUnwrap(
            JSONSerialization.jsonObject(with: body) as? [String: Any]
        )
        XCTAssertEqual(json["repo"] as? String, model.repository)
        XCTAssertEqual(json["provider"] as? String, "qwen3-asr-mlx")
        XCTAssertEqual(json["directory"] as? String, model.directoryName)
        XCTAssertEqual(json["auto_load"] as? Bool, true)
        XCTAssertEqual(json["files"] as? [String], model.files)
    }

    func testKokoroRecommendationIncludesEveryPublishedVoice() throws {
        let model = try XCTUnwrap(
            RecommendedModel.builtIns.first { $0.provider == "kokoro-mlx" }
        )
        let voices = model.files.filter { $0.hasPrefix("voices/") }
        let namedVoices = Set(voices.filter { !$0.contains("/zf_") && !$0.contains("/zm_") })
        let numberedVoices = Set(voices.compactMap { path -> Int? in
            let stem = URL(fileURLWithPath: path).deletingPathExtension().lastPathComponent
            guard stem.hasPrefix("zf_") || stem.hasPrefix("zm_") else { return nil }
            return Int(stem.dropFirst(3))
        })

        XCTAssertEqual(voices.count, 103)
        XCTAssertEqual(Set(voices).count, 103)
        XCTAssertEqual(namedVoices, [
            "voices/af_maple.safetensors",
            "voices/af_sol.safetensors",
            "voices/bf_vale.safetensors",
        ])
        XCTAssertEqual(numberedVoices, Set(1...100))
        XCTAssertEqual(model.estimatedSizeBytes, 380_917_492)
    }

    func testDirectoryRecommendationFindsCompleteModelUnderAlternateFolderName() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("macai-recommendation-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: root) }
        let actual = root.appendingPathComponent("publisher-model-name", isDirectory: true)
        try FileManager.default.createDirectory(
            at: actual.appendingPathComponent("tokenizer", isDirectory: true),
            withIntermediateDirectories: true
        )
        try Data("{}".utf8).write(to: actual.appendingPathComponent("config.json"))
        try Data("weights".utf8).write(to: actual.appendingPathComponent("model.safetensors"))
        try Data("tokens".utf8).write(to: actual.appendingPathComponent("tokenizer/vocab.json"))

        let model = RecommendedModel(
            id: "test-recommended",
            title: "Test",
            summary: "Test",
            modelType: "stt",
            provider: "test-provider",
            repository: "owner/model",
            files: ["config.json", "model.safetensors", "tokenizer/vocab.json"],
            directoryName: "preferred-folder-name",
            estimatedSizeBytes: 1
        )
        let repositoryModel = RepoModel(
            fileName: actual.lastPathComponent,
            modelType: model.modelType,
            provider: model.provider,
            path: actual.path,
            sizeBytes: model.estimatedSizeBytes
        )

        XCTAssertEqual(model.downloadedURL(in: root)?.standardizedFileURL, actual.standardizedFileURL)
        XCTAssertTrue(model.matches(repositoryModel: repositoryModel))

        try FileManager.default.removeItem(at: actual.appendingPathComponent("tokenizer/vocab.json"))
        XCTAssertNil(model.downloadedURL(in: root))
        XCTAssertFalse(model.matches(repositoryModel: repositoryModel))
    }
}

private final class RecommendationURLProtocol: URLProtocol {
    static var requestBody: Data?

    override class func canInit(with request: URLRequest) -> Bool { true }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.requestBody = request.httpBody ?? request.httpBodyStream.flatMap(Self.readBody)
        guard let url = request.url,
              let response = HTTPURLResponse(
                url: url,
                statusCode: 200,
                httpVersion: "HTTP/1.1",
                headerFields: ["Content-Type": "application/json"]
              ) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(
            self,
            didLoad: Data(#"{"id":"qwen3-asr-mlx-8bit","state":"ready"}"#.utf8)
        )
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}

    private static func readBody(from stream: InputStream) -> Data? {
        stream.open()
        defer { stream.close() }
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4_096)
        while stream.hasBytesAvailable {
            let count = stream.read(&buffer, maxLength: buffer.count)
            guard count >= 0 else { return nil }
            if count == 0 { break }
            data.append(buffer, count: count)
        }
        return data
    }
}
#endif
