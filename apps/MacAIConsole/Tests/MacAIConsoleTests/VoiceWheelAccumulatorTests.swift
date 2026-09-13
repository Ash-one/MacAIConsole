#if canImport(XCTest)
import XCTest
@testable import MacAIConsole

final class VoiceWheelAccumulatorTests: XCTestCase {
    private var accumulator = WheelStepAccumulator()

    override func setUp() {
        super.setUp()
        accumulator = WheelStepAccumulator()
    }

    // MARK: - 触控板路径（事件携带 gesture phase，行为保持不变）

    /// 触控板精确滚动：delta 累积过阈值才记一步，残余带入后续事件。
    func testPreciseDeltaAccumulatesToThreshold() {
        func trackpad(_ deltaY: CGFloat) -> Int {
            accumulator.stepCount(
                forDeltaY: deltaY, hasPreciseScrollingDeltas: true,
                isMomentum: false, hasGesturePhase: true, gestureTimestamp: 0
            )
        }
        // 累积 4、8、12 → 第三次过阈值记一步（向上），残余 2 继续带入。
        XCTAssertEqual(trackpad(4), 0)
        XCTAssertEqual(trackpad(4), 0)
        XCTAssertEqual(trackpad(4), -1)
        XCTAssertEqual(trackpad(4), 0)
        // 残余 2 + 4 + 4 = 10 → 再记一步。
        XCTAssertEqual(trackpad(4), -1)
    }

    /// 向下滚动（deltaY < 0）产生正值步进 = 列表下一项。
    func testDownwardScrollYieldsNextItem() {
        func trackpad(_ deltaY: CGFloat, at timestamp: TimeInterval) -> Int {
            accumulator.stepCount(
                forDeltaY: deltaY, hasPreciseScrollingDeltas: true,
                isMomentum: false, hasGesturePhase: true, gestureTimestamp: timestamp
            )
        }
        XCTAssertEqual(trackpad(-4, at: 0), 0)
        XCTAssertEqual(trackpad(-4, at: 0.016), 0)
        XCTAssertEqual(trackpad(-4, at: 0.032), 1)
    }

    /// 惯性事件整体丢弃，且不影响后续事件的累积残余。
    func testMomentumEventsAreDropped() {
        func trackpad(_ deltaY: CGFloat, momentum: Bool) -> Int {
            accumulator.stepCount(
                forDeltaY: deltaY, hasPreciseScrollingDeltas: true,
                isMomentum: momentum, hasGesturePhase: true, gestureTimestamp: 0
            )
        }
        XCTAssertEqual(trackpad(100, momentum: true), 0)
        // 若惯性参与了累积，这次 +4 会直接过阈值；正确行为是残余仍从 0 起算。
        XCTAssertEqual(trackpad(4, momentum: false), 0)
        XCTAssertEqual(trackpad(4, momentum: false), 0)
        // 残余 4 + 4 + 4 = 12 过阈值 → 记一步。
        XCTAssertEqual(trackpad(4, momentum: false), -1)
    }

    /// 触控板零 delta 不产生步进也不污染残余。
    func testTrackpadZeroDeltaIsNoop() {
        func trackpad(_ deltaY: CGFloat) -> Int {
            accumulator.stepCount(
                forDeltaY: deltaY, hasPreciseScrollingDeltas: true,
                isMomentum: false, hasGesturePhase: true, gestureTimestamp: 0
            )
        }
        XCTAssertEqual(trackpad(0), 0)
        XCTAssertEqual(trackpad(12), -1)
    }

    // MARK: - 滚轮类路径（phase 为空：一次物理滚动固定一行）

    /// 刻意的独立滚动（间隔 > 突发窗口）每次固定步进一行，向下滚 = 下一项。
    func testDeliberateWheelRollsStepOneRowEach() {
        func wheel(_ deltaY: CGFloat, at timestamp: TimeInterval) -> Int {
            accumulator.stepCount(
                forDeltaY: deltaY, hasPreciseScrollingDeltas: false,
                isMomentum: false, hasGesturePhase: false, gestureTimestamp: timestamp
            )
        }
        XCTAssertEqual(wheel(-1, at: 1.0), 1)
        XCTAssertEqual(wheel(-1, at: 1.3), 1)
        XCTAssertEqual(wheel(1, at: 1.6), -1)
    }

    /// 高分辨率滚轮把一个 notch 拆成多个精确 delta 事件：整个突发只步进一行，
    /// 这是「一次滚动跳很多行」缺陷的回归测试。
    func testHighResolutionWheelBurstStepsOncePerRoll() {
        func wheel(_ deltaY: CGFloat, at timestamp: TimeInterval) -> Int {
            accumulator.stepCount(
                forDeltaY: deltaY, hasPreciseScrollingDeltas: true,
                isMomentum: false, hasGesturePhase: false, gestureTimestamp: timestamp
            )
        }
        // 同一 notch 的突发（事件间隔 < 窗口），累计 delta 远超触控板阈值。
        XCTAssertEqual(wheel(-4, at: 2.000), 1)
        XCTAssertEqual(wheel(-4, at: 2.008), 0)
        XCTAssertEqual(wheel(-4, at: 2.016), 0)
        XCTAssertEqual(wheel(-4, at: 2.024), 0)
        XCTAssertEqual(wheel(-4, at: 2.032), 0)
    }

    /// 突发结束后（窗口内无事件）重新武装：下一次独立滚动再步进一行；
    /// 窗口内的后续事件仍属于同一次滚动，被吸收。
    func testWheelBurstRearmsAfterQuietGap() {
        func wheel(_ deltaY: CGFloat, at timestamp: TimeInterval) -> Int {
            accumulator.stepCount(
                forDeltaY: deltaY, hasPreciseScrollingDeltas: true,
                isMomentum: false, hasGesturePhase: false, gestureTimestamp: timestamp
            )
        }
        XCTAssertEqual(wheel(-4, at: 3.000), 1)
        XCTAssertEqual(wheel(-4, at: 3.050), 0)
        // 距上次事件 0.15s ≥ 突发窗口 → 新的一次滚动。
        XCTAssertEqual(wheel(-4, at: 3.200), 1)
        XCTAssertEqual(wheel(-4, at: 3.250), 0)
    }

    /// 滚轮路径零 delta 是无副作用 no-op：不步进、不消耗突发状态。
    func testWheelZeroDeltaDoesNotConsumeBurst() {
        func wheel(_ deltaY: CGFloat, at timestamp: TimeInterval) -> Int {
            accumulator.stepCount(
                forDeltaY: deltaY, hasPreciseScrollingDeltas: true,
                isMomentum: false, hasGesturePhase: false, gestureTimestamp: timestamp
            )
        }
        XCTAssertEqual(wheel(-4, at: 4.000), 1)
        XCTAssertEqual(wheel(0, at: 4.050), 0)
        // 零 delta 若推进了时间戳，这里的 -4 会被当作新滚动；正确行为是吸收。
        XCTAssertEqual(wheel(-4, at: 4.060), 0)
    }
}
#endif
