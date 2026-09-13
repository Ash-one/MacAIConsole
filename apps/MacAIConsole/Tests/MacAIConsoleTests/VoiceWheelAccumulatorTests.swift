#if canImport(XCTest)
import XCTest
@testable import MacAIConsole

final class VoiceWheelAccumulatorTests: XCTestCase {
    private var accumulator = WheelStepAccumulator()

    override func setUp() {
        super.setUp()
        accumulator = WheelStepAccumulator()
    }

    /// 鼠标滚轮（离散 delta，以行为单位）：每个 notch 一步，向下滚 = 下一项。
    func testDiscreteWheelTickStepsOneItemPerNotch() {
        XCTAssertEqual(accumulator.stepCount(forDeltaY: -1, hasPreciseScrollingDeltas: false, isMomentum: false), 1)
        XCTAssertEqual(accumulator.stepCount(forDeltaY: -1, hasPreciseScrollingDeltas: false, isMomentum: false), 1)
        XCTAssertEqual(accumulator.stepCount(forDeltaY: 1, hasPreciseScrollingDeltas: false, isMomentum: false), -1)
    }

    /// 触控板精确滚动：delta 累积过阈值才记一步，残余带入后续事件。
    func testPreciseDeltaAccumulatesToThreshold() {
        let precise = (deltaY: CGFloat(4), precise: true, momentum: false)
        // 累积 4、8、12 → 第三次过阈值记一步（向上），残余 2 继续带入。
        XCTAssertEqual(accumulator.stepCount(forDeltaY: precise.deltaY, hasPreciseScrollingDeltas: precise.precise, isMomentum: precise.momentum), 0)
        XCTAssertEqual(accumulator.stepCount(forDeltaY: precise.deltaY, hasPreciseScrollingDeltas: precise.precise, isMomentum: precise.momentum), 0)
        XCTAssertEqual(accumulator.stepCount(forDeltaY: precise.deltaY, hasPreciseScrollingDeltas: precise.precise, isMomentum: precise.momentum), -1)
        XCTAssertEqual(accumulator.stepCount(forDeltaY: precise.deltaY, hasPreciseScrollingDeltas: precise.precise, isMomentum: precise.momentum), 0)
        // 残余 2 + 4 + 4 = 10 → 再记一步。
        XCTAssertEqual(accumulator.stepCount(forDeltaY: precise.deltaY, hasPreciseScrollingDeltas: precise.precise, isMomentum: precise.momentum), -1)
    }

    /// 向下滚动（deltaY < 0）产生正值步进 = 列表下一项。
    func testDownwardScrollYieldsNextItem() {
        XCTAssertEqual(accumulator.stepCount(forDeltaY: -4, hasPreciseScrollingDeltas: true, isMomentum: false), 0)
        XCTAssertEqual(accumulator.stepCount(forDeltaY: -4, hasPreciseScrollingDeltas: true, isMomentum: false), 0)
        XCTAssertEqual(accumulator.stepCount(forDeltaY: -4, hasPreciseScrollingDeltas: true, isMomentum: false), 1)
    }

    /// 惯性事件整体丢弃，且不影响后续事件的累积残余。
    func testMomentumEventsAreDropped() {
        XCTAssertEqual(accumulator.stepCount(forDeltaY: 100, hasPreciseScrollingDeltas: true, isMomentum: true), 0)
        // 若惯性参与了累积，这次 +4 会直接过阈值；正确行为是残余仍从 0 起算。
        XCTAssertEqual(accumulator.stepCount(forDeltaY: 4, hasPreciseScrollingDeltas: true, isMomentum: false), 0)
        XCTAssertEqual(accumulator.stepCount(forDeltaY: 4, hasPreciseScrollingDeltas: true, isMomentum: false), 0)
        // 残余 4 + 4 + 4 = 12 过阈值 → 记一步。
        XCTAssertEqual(accumulator.stepCount(forDeltaY: 4, hasPreciseScrollingDeltas: true, isMomentum: false), -1)
    }

    /// 零 delta 不产生步进也不污染残余。
    func testZeroDeltaIsNoop() {
        XCTAssertEqual(accumulator.stepCount(forDeltaY: 0, hasPreciseScrollingDeltas: true, isMomentum: false), 0)
        XCTAssertEqual(accumulator.stepCount(forDeltaY: 12, hasPreciseScrollingDeltas: true, isMomentum: false), -1)
    }
}
#endif
