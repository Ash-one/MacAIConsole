#if canImport(XCTest)
import AppKit
import XCTest
@testable import MacAIConsole

final class LogConsoleViewTests: XCTestCase {
    func testInfoTextColorTracksLightAndDarkAppearance() throws {
        let entry = LogEntry(id: 0, level: .info, text: "INFO ready")
        let lightAppearance = try XCTUnwrap(NSAppearance(named: .aqua))
        let darkAppearance = try XCTUnwrap(NSAppearance(named: .darkAqua))

        let lightText = LogConsoleView.render([entry], appearance: lightAppearance)
        let darkText = LogConsoleView.render([entry], appearance: darkAppearance)
        let lightColor = try XCTUnwrap(lightText.attribute(
            .foregroundColor,
            at: 0,
            effectiveRange: nil
        ) as? NSColor)
        let darkColor = try XCTUnwrap(darkText.attribute(
            .foregroundColor,
            at: 0,
            effectiveRange: nil
        ) as? NSColor)

        XCTAssertLessThan(lightColor.brightnessComponent, darkColor.brightnessComponent)
        XCTAssertEqual(lightColor.alphaComponent, darkColor.alphaComponent, accuracy: 0.001)
    }
}
#endif
