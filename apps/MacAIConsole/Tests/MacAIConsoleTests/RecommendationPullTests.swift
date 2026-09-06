#if canImport(XCTest)
import Foundation
import XCTest
@testable import MacAIConsole

final class RecommendationPullTests: XCTestCase {
    override func tearDown() {
        RecommendationURLProtocol.requestBody = nil
        super.tearDown()
    }

    func testProfilePullSendsOnlyProfileIdentityAndIntent() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [RecommendationURLProtocol.self]
        let session = URLSession(configuration: configuration)
        let api = DaemonAPI(
            baseURL: try XCTUnwrap(URL(string: "http://127.0.0.1:11435")),
            session: session
        )
        let response = try await api.pullProfile("org.example.future-model", autoLoad: true)

        XCTAssertEqual(response.id, "Qwen3-ASR-0.6B-MLX-4bit")
        let body = try XCTUnwrap(RecommendationURLProtocol.requestBody)
        let json = try XCTUnwrap(
            JSONSerialization.jsonObject(with: body) as? [String: Any]
        )
        XCTAssertEqual(RecommendationURLProtocol.requestPath, "/api/model-profiles/org.example.future-model/pull")
        XCTAssertEqual(json["auto_load"] as? Bool, true)
        XCTAssertEqual(json.count, 1)
    }
}

private final class RecommendationURLProtocol: URLProtocol {
    static var requestBody: Data?
    static var requestPath: String?

    override class func canInit(with request: URLRequest) -> Bool { true }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.requestBody = request.httpBody ?? request.httpBodyStream.flatMap(Self.readBody)
        Self.requestPath = request.url?.path
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
            didLoad: Data(#"{"id":"Qwen3-ASR-0.6B-MLX-4bit","state":"ready"}"#.utf8)
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
