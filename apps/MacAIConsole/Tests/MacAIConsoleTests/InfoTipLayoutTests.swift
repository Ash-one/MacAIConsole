import XCTest
@testable import MacAIConsole

final class InfoTipLayoutTests: XCTestCase {
    func testTooltipIsCenteredDirectlyBelowTrigger() {
        let origin = InfoTip.Layout.origin(
            bubbleSize: CGSize(width: 200, height: 60),
            target: CGRect(x: 240, y: 180, width: 20, height: 20),
            containerSize: CGSize(width: 680, height: 600)
        )

        XCTAssertEqual(origin, CGPoint(x: 150, y: 208))
    }

    func testTooltipBelowPlacementDoesNotDependOnAvailableSpaceAbove() {
        let origin = InfoTip.Layout.origin(
            bubbleSize: CGSize(width: 320, height: 80),
            target: CGRect(x: 160, y: 30, width: 20, height: 20),
            containerSize: CGSize(width: 680, height: 600)
        )

        XCTAssertEqual(origin, CGPoint(x: 12, y: 58))
    }

    func testTooltipStaysInsideHorizontalViewportMargins() {
        let leftOrigin = InfoTip.Layout.origin(
            bubbleSize: CGSize(width: 260, height: 60),
            target: CGRect(x: 8, y: 180, width: 20, height: 20),
            containerSize: CGSize(width: 680, height: 600)
        )
        let rightOrigin = InfoTip.Layout.origin(
            bubbleSize: CGSize(width: 260, height: 60),
            target: CGRect(x: 660, y: 180, width: 20, height: 20),
            containerSize: CGSize(width: 680, height: 600)
        )

        XCTAssertEqual(leftOrigin.x, 12)
        XCTAssertEqual(rightOrigin.x, 408)
    }
}
