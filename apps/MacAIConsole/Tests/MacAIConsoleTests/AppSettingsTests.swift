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

    func testQwen3ASR06BIsDisabledByDefault() {
        XCTAssertFalse(AppSettings.qwen3ASR06BEnabled(in: defaults))
    }

    func testQwen3ASR06BPreferenceLoadsSavedValue() {
        defaults.set(true, forKey: AppSettings.qwen3ASR06BEnabledKey)

        let reloadedDefaults = UserDefaults(suiteName: suiteName)!
        XCTAssertTrue(AppSettings.qwen3ASR06BEnabled(in: reloadedDefaults))

        reloadedDefaults.set(false, forKey: AppSettings.qwen3ASR06BEnabledKey)
        let secondReload = UserDefaults(suiteName: suiteName)!
        XCTAssertFalse(AppSettings.qwen3ASR06BEnabled(in: secondReload))
    }
}