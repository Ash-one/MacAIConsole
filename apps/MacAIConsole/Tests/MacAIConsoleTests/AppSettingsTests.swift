import Foundation
import XCTest
@testable import MacAIConsole

final class AppSettingsTests: XCTestCase {
    private var defaults: UserDefaults!
    private var suiteName: String!

    override func setUp() {
        super.setUp()
        suiteName = "MacAIConsoleTests.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suiteName)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suiteName)
        defaults = nil
        suiteName = nil
        super.tearDown()
    }

    func testProxyDefaultsToSystemAndLoadsSavedMode() {
        XCTAssertEqual(AppSettings.proxyMode(in: defaults), .system)
        defaults.set(ProxyMode.manual.rawValue, forKey: AppSettings.proxyModeKey)
        XCTAssertEqual(AppSettings.proxyMode(in: defaults), .manual)
    }

    func testAppearanceDefaultsToSystemAndLoadsSavedMode() {
        XCTAssertEqual(AppSettings.appearance(in: defaults), .system)
        defaults.set(AppearanceMode.dark.rawValue, forKey: AppSettings.appearanceKey)
        XCTAssertEqual(AppSettings.appearance(in: defaults), .dark)
        defaults.set("unsupported", forKey: AppSettings.appearanceKey)
        XCTAssertEqual(AppSettings.appearance(in: defaults), .system)
    }

    func testProxyURLNormalizationAcceptsNewbieFriendlyHostAndRejectsInvalidSchemes() {
        XCTAssertEqual(
            AppSettings.normalizedProxyURL("127.0.0.1:6152"),
            "http://127.0.0.1:6152"
        )
        XCTAssertEqual(
            AppSettings.normalizedProxyURL(" https://proxy.example:8443 "),
            "https://proxy.example:8443"
        )
        XCTAssertNil(AppSettings.normalizedProxyURL("socks5://127.0.0.1:6153"))
        XCTAssertNil(AppSettings.normalizedProxyURL("   "))
    }

    func testDownloadSourceDefaultsToOfficialAndLoadsSavedMode() {
        XCTAssertEqual(AppSettings.downloadSource(in: defaults), .official)
        defaults.set(ModelDownloadSource.hfMirror.rawValue, forKey: AppSettings.downloadSourceKey)
        XCTAssertEqual(AppSettings.downloadSource(in: defaults), .hfMirror)
    }

    func testIgnoredRecommendationsPersistWithoutDuplicates() {
        XCTAssertTrue(AppSettings.ignoredRecommendationIDs(in: defaults).isEmpty)

        AppSettings.ignoreRecommendation("org.macai.test", in: defaults)
        AppSettings.ignoreRecommendation("org.macai.test", in: defaults)

        XCTAssertEqual(AppSettings.ignoredRecommendationIDs(in: defaults), ["org.macai.test"])
    }

    func testIgnoredItemsCanBeRestoredByCategory() {
        AppSettings.ignoreRecommendation("org.macai.model", in: defaults)
        AppSettings.ignoreRunner("org.macai.runner", in: defaults)

        AppSettings.showAllRecommendations(in: defaults)
        XCTAssertTrue(AppSettings.ignoredRecommendationIDs(in: defaults).isEmpty)
        XCTAssertEqual(AppSettings.ignoredRunnerIDs(in: defaults), ["org.macai.runner"])

        AppSettings.showAllRunners(in: defaults)
        XCTAssertTrue(AppSettings.ignoredRunnerIDs(in: defaults).isEmpty)
    }

    func testDownloadEndpointNormalizationAndSelection() {
        XCTAssertEqual(
            AppSettings.normalizedDownloadEndpoint(" hf-mirror.com/ "),
            "https://hf-mirror.com"
        )
        XCTAssertEqual(
            AppSettings.downloadEndpoint(for: .official, customEndpoint: "ignored"),
            "https://huggingface.co"
        )
        XCTAssertEqual(
            AppSettings.downloadEndpoint(for: .custom, customEndpoint: "https://mirror.example/hf/"),
            "https://mirror.example/hf"
        )
        XCTAssertNil(AppSettings.normalizedDownloadEndpoint("https://user:pass@mirror.example"))
        XCTAssertNil(AppSettings.normalizedDownloadEndpoint("https://mirror.example?token=secret"))
    }
}
