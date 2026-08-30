#if canImport(XCTest)
import XCTest
@testable import MacAIConsole

final class FormatTests: XCTestCase {
    func testUptimeBuckets() {
        XCTAssertEqual(Format.uptime(59), "59 秒")
        XCTAssertEqual(Format.uptime(60), "1 分 0 秒")
        XCTAssertEqual(Format.uptime(3_661), "1 小时 1 分")
        XCTAssertEqual(Format.uptime(86_400), "1 天 0 小时")
        XCTAssertEqual(Format.uptime(90_061), "1 天 1 小时")
    }

    func testDurationBuckets() {
        XCTAssertEqual(Format.duration(milliseconds: nil), "—")
        XCTAssertEqual(Format.duration(milliseconds: 999), "999 ms")
        XCTAssertEqual(Format.duration(milliseconds: 1_000), "1.0 秒")
        XCTAssertEqual(Format.duration(milliseconds: 59_999), "60.0 秒")
        XCTAssertEqual(Format.duration(milliseconds: 60_000), "1 分 0 秒")
        XCTAssertEqual(Format.duration(milliseconds: 3_661_000), "61 分 1 秒")
    }

    func testRealTimeFactor() {
        XCTAssertEqual(Format.realTimeFactor(nil), "—")
        XCTAssertEqual(Format.realTimeFactor(0.0604), "0.06×")
        XCTAssertEqual(Format.realTimeFactor(1.0), "1.00×")
    }

    // ByteCountFormatter / RelativeDateTimeFormatter 的输出随系统语言变化，
    // 只有 nil 哨兵值是确定性契约。
    func testNilSentinelsRenderAsEmDash() {
        XCTAssertEqual(Format.bytes(nil), "—")
        XCTAssertEqual(Format.relativeTime(nil), "—")
        XCTAssertEqual(Format.relativeTime(milliseconds: nil), "—")
        XCTAssertEqual(Format.absoluteTime(milliseconds: nil), "—")
        XCTAssertFalse(Format.bytes(1024).isEmpty)
    }

    func testRunningDurationClampsFutureStartToZero() {
        let now = UInt64(Date().timeIntervalSince1970 * 1_000)
        XCTAssertEqual(Format.runningDuration(startedAtMs: now + 60_000), "0 ms")
    }
}
#endif
