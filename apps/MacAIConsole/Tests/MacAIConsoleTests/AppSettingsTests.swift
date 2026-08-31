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
}