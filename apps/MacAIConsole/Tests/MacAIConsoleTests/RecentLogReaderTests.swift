#if canImport(XCTest)
import XCTest
@testable import MacAIConsole

final class RecentLogReaderTests: XCTestCase {
    func testReadsRecentLinesStripsANSIAndParsesLevels() throws {
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("macai-log-reader-\(UUID().uuidString).log")
        defer { try? FileManager.default.removeItem(at: url) }

        let content = [
            "discarded",
            "2026-08-27T00:00:00Z \u{001B}[36mDEBUG\u{001B}[0m request",
            "2026-08-27T00:00:01Z INFO ready",
            "2026-08-27T00:00:02Z WARN slow",
            "2026-08-27T00:00:03Z ERROR failed",
            "",
        ].joined(separator: "\n")
        try Data(content.utf8).write(to: url)

        let snapshot = try RecentLogReader.read(url: url, maxLines: 4)

        XCTAssertEqual(snapshot.entries.map(\.text), [
            "2026-08-27T00:00:00Z DEBUG request",
            "2026-08-27T00:00:01Z INFO ready",
            "2026-08-27T00:00:02Z WARN slow",
            "2026-08-27T00:00:03Z ERROR failed",
        ])
        XCTAssertEqual(snapshot.entries.map(\.level), [.debug, .info, .warning, .error])
        XCTAssertEqual(snapshot.entries.filter(LogLevel.info.includes).map(\.level), [.info, .warning, .error])
        XCTAssertEqual(snapshot.lineCount, 4)
        XCTAssertNotNil(snapshot.modifiedAt)
    }
}
#endif
