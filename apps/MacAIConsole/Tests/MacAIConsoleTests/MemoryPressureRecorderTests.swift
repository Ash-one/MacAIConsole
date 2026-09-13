#if canImport(XCTest)
import Foundation
import XCTest
@testable import MacAIConsole

final class MemoryPressureRecorderTests: XCTestCase {
    private let gb: UInt64 = 1 << 30

    /// 受控时钟：手动推进，保证采样时间戳确定性。
    private final class ManualClock {
        var now = Date(timeIntervalSince1970: 1_000_000)
        func advance(_ interval: TimeInterval) { now = now.addingTimeInterval(interval) }
    }

    private func makeRecorder() -> (recorder: MemoryPressureRecorder, clock: ManualClock) {
        let clock = ManualClock()
        return (MemoryPressureRecorder(clock: { clock.now }), clock)
    }

    /// 模拟一个在线轮询周期：记录一次读数后推进 2s 节拍。
    private func tick(
        _ recorder: inout MemoryPressureRecorder,
        _ clock: ManualClock,
        used: UInt64,
        total: UInt64
    ) {
        clock.advance(2)
        recorder.record(usedBytes: used, totalBytes: total)
    }

    func testRecordComputesFractionsFromByteCounts() {
        var (recorder, clock) = makeRecorder()
        tick(&recorder, clock, used: 16 * gb, total: 32 * gb)
        XCTAssertEqual(recorder.samples.count, 1)
        XCTAssertEqual(recorder.samples[0].usedFraction, 0.5, accuracy: 1e-12)
        XCTAssertEqual(recorder.samples[0].timestamp, clock.now)
    }

    func testMissingOrInvalidReadingsProduceNoSample() {
        var (recorder, _) = makeRecorder()
        recorder.record(usedBytes: nil, totalBytes: 32 * gb)
        recorder.record(usedBytes: 16 * gb, totalBytes: nil)
        recorder.record(usedBytes: 16 * gb, totalBytes: 0)
        XCTAssertTrue(recorder.samples.isEmpty)
    }

    func testUsedFractionClampsAtOne() {
        var (recorder, clock) = makeRecorder()
        tick(&recorder, clock, used: 40 * gb, total: 32 * gb)
        XCTAssertEqual(recorder.samples[0].usedFraction, 1.0, accuracy: 1e-12)
    }

    func testCapacityEvictionDropsOldestSamples() {
        var (recorder, clock) = makeRecorder()
        // 索引 i 的占用率为 i/1000，用于辨识最旧样本是否被淘汰。
        for index in 0...MemoryPressureRecorder.capacity {
            tick(&recorder, clock, used: UInt64(index), total: 1000)
        }
        XCTAssertEqual(recorder.samples.count, MemoryPressureRecorder.capacity)
        XCTAssertEqual(
            recorder.samples.first?.usedFraction ?? -1,
            1.0 / 1000,
            accuracy: 1e-12
        )
    }

    func testDisplayWindowKeepsSamplesRelativeToLatestTimestamp() {
        var (recorder, clock) = makeRecorder()
        // 2s 节拍记满 300 容量（约 10 分钟），显示窗口只保留最新 300s。
        for _ in 0..<MemoryPressureRecorder.capacity {
            tick(&recorder, clock, used: 16 * gb, total: 32 * gb)
        }
        let windowed = MemoryPressureRecorder.displayWindowSamples(of: recorder.samples)
        // 窗口 300s、间隔 2s：两端含端点共 151 个样本。
        XCTAssertEqual(windowed.count, 151)
        XCTAssertEqual(windowed.last, recorder.samples.last)
    }

    func testDisplayWindowHandlesEmptyAndSingleSample() {
        XCTAssertTrue(MemoryPressureRecorder.displayWindowSamples(of: []).isEmpty)

        var (recorder, clock) = makeRecorder()
        tick(&recorder, clock, used: 16 * gb, total: 32 * gb)
        XCTAssertEqual(MemoryPressureRecorder.displayWindowSamples(of: recorder.samples).count, 1)
    }
}
#endif
