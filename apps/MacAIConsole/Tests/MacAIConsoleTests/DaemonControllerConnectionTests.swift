#if canImport(XCTest)
import Foundation
import XCTest
@testable import MacAIConsole

@MainActor
final class DaemonControllerConnectionTests: XCTestCase {
    func testRuntimeTimeoutKeepsHealthyDaemonOnline() async throws {
        let controller = try makeController()

        _ = await controller.tick()
        XCTAssertEqual(controller.phase, .online)
        XCTAssertNil(controller.lastError)

        _ = await controller.tick()
        XCTAssertEqual(controller.phase, .online)
        XCTAssertNil(controller.lastError)
    }

    func testSuccessfulMutationIsNotReportedFailedWhenRefreshTimesOut() async throws {
        let controller = try makeController()

        await controller.rename("test-old", to: "test-new")

        XCTAssertNil(controller.lastError)
    }

    private func makeController() throws -> DaemonController {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [BusyRuntimeURLProtocol.self]
        let session = URLSession(configuration: configuration)
        let baseURL = try XCTUnwrap(URL(string: "http://127.0.0.1:11435"))
        let api = DaemonAPI(
            baseURL: baseURL,
            session: session
        )
        return DaemonController(api: api, logsEnabled: false)
    }
}

private final class BusyRuntimeURLProtocol: URLProtocol {
    override class func canInit(with request: URLRequest) -> Bool {
        true
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest {
        request
    }

    override func startLoading() {
        guard let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }

        if url.path == "/api/runtime" {
            client?.urlProtocol(self, didFailWithError: URLError(.timedOut))
            return
        }

        let body: String
        switch url.path {
        case "/health":
            body = #"{"status":"ok","version":"test","model_count":1}"#
        case "/v1/models", "/api/providers":
            body = #"{"data":[]}"#
        case "/api/tasks":
            body = #"{"running":[],"completed":[]}"#
        case "/api/logging":
            body = #"{"level":"info"}"#
        case let path where path.hasPrefix("/api/models/") && path.hasSuffix("/rename"):
            body = "{}"
        default:
            client?.urlProtocol(self, didFailWithError: URLError(.unsupportedURL))
            return
        }

        guard let response = HTTPURLResponse(
            url: url,
            statusCode: 200,
            httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        ) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}
#endif
