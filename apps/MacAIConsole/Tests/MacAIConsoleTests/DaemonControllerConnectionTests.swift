#if canImport(XCTest)
import Foundation
import XCTest
@testable import MacAIConsole

@MainActor
final class DaemonControllerConnectionTests: XCTestCase {
    func testSystemProxySettingsArePassedToDaemonWithoutOverridingExplicitEnvironment() {
        let settings: [String: Any] = [
            "HTTPEnable": 1,
            "HTTPProxy": "127.0.0.1",
            "HTTPPort": 6152,
            "HTTPSEnable": 1,
            "HTTPSProxy": "127.0.0.1",
            "HTTPSPort": 6152,
        ]

        let inferred = DaemonController.environmentByApplyingSystemProxy(
            settings,
            to: ["PATH": "/usr/bin"]
        )
        XCTAssertEqual(inferred["HTTP_PROXY"], "http://127.0.0.1:6152")
        XCTAssertEqual(inferred["HTTPS_PROXY"], "http://127.0.0.1:6152")
        XCTAssertEqual(inferred["NO_PROXY"], "127.0.0.1,localhost,::1")

        let explicit = DaemonController.environmentByApplyingSystemProxy(
            settings,
            to: [
                "HTTPS_PROXY": "http://proxy.example:8080",
                "NO_PROXY": "example.internal",
            ]
        )
        XCTAssertEqual(explicit["HTTPS_PROXY"], "http://proxy.example:8080")
        XCTAssertEqual(explicit["NO_PROXY"], "example.internal,127.0.0.1,localhost,::1")
    }

    func testManualAndDisabledProxyModesOverrideInheritedProxyEnvironment() {
        let inherited = [
            "HTTP_PROXY": "http://old.example:8080",
            "HTTPS_PROXY": "http://old.example:8080",
            "ALL_PROXY": "socks5://old.example:1080",
        ]
        let manual = DaemonController.environmentByApplyingProxyMode(
            .manual,
            httpProxy: "http://127.0.0.1:6152",
            httpsProxy: "http://127.0.0.1:6152",
            systemSettings: nil,
            to: inherited
        )
        XCTAssertEqual(manual["HTTP_PROXY"], "http://127.0.0.1:6152")
        XCTAssertEqual(manual["HTTPS_PROXY"], "http://127.0.0.1:6152")
        XCTAssertNil(manual["ALL_PROXY"])

        let disabled = DaemonController.environmentByApplyingProxyMode(
            .disabled,
            httpProxy: nil,
            httpsProxy: nil,
            systemSettings: nil,
            to: inherited
        )
        XCTAssertNil(disabled["HTTP_PROXY"])
        XCTAssertNil(disabled["HTTPS_PROXY"])
        XCTAssertNil(disabled["ALL_PROXY"])
    }

    func testSelectedDownloadSourceIsPassedToDaemonEnvironment() {
        let inherited = [
            AppSettings.downloadEndpointEnvironmentKey: "https://old.example"
        ]

        let mirror = DaemonController.environmentByApplyingDownloadSource(
            .hfMirror,
            customEndpoint: nil,
            to: inherited
        )
        XCTAssertEqual(
            mirror[AppSettings.downloadEndpointEnvironmentKey],
            "https://hf-mirror.com"
        )

        let custom = DaemonController.environmentByApplyingDownloadSource(
            .custom,
            customEndpoint: "mirror.example/hf/",
            to: inherited
        )
        XCTAssertEqual(
            custom[AppSettings.downloadEndpointEnvironmentKey],
            "https://mirror.example/hf"
        )

        let official = DaemonController.environmentByApplyingDownloadSource(
            .official,
            customEndpoint: nil,
            to: inherited
        )
        XCTAssertNil(official[AppSettings.downloadEndpointEnvironmentKey])
    }

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

    func testEnrichedPathPreservesCurrentAndAppendsExisting() {
        let initial = "/usr/bin:/bin"
        let enriched = DaemonController.enrichedPath(current: initial)
        XCTAssertTrue(enriched.hasPrefix("/usr/bin:/bin"))
    }

    func testEnvironmentForSpawningSetsRunnersAndUvVariables() {
        let tempDir = FileManager.default.temporaryDirectory.appendingPathComponent("test_spawn_\(UUID().uuidString)")
        let bundleURL = tempDir.appendingPathComponent("MacAIConsole.app")
        let macosURL = bundleURL.appendingPathComponent("Contents/MacOS")
        let resURL = bundleURL.appendingPathComponent("Contents/Resources")
        let runnersURL = resURL.appendingPathComponent("runners")
        let fakeUv = macosURL.appendingPathComponent("uv")

        try? FileManager.default.createDirectory(at: macosURL, withIntermediateDirectories: true)
        try? FileManager.default.createDirectory(at: runnersURL, withIntermediateDirectories: true)
        FileManager.default.createFile(atPath: fakeUv.path, contents: Data(), attributes: [.posixPermissions: 0o755])
        defer { try? FileManager.default.removeItem(at: tempDir) }

        guard let testBundle = Bundle(url: bundleURL) else {
            XCTFail("Failed to create test bundle")
            return
        }

        let env = DaemonController.environmentForSpawning(
            base: ["PATH": "/usr/bin:/bin"],
            systemSettings: nil,
            bundle: testBundle
        )

        XCTAssertEqual(env["MACAI_RUNNERS_DIR"], runnersURL.path)
        XCTAssertEqual(env["MACAI_UV_PATH"], fakeUv.path)
        XCTAssertNotNil(env["PATH"])
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
