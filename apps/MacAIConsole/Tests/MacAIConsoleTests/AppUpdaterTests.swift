import XCTest
@testable import MacAIConsole

final class AppUpdaterTests: XCTestCase {
    func testVersionComparisonToleratesVPrefixAndMissingSegments() {
        XCTAssertEqual(AppUpdater.compareVersions("v0.2.0", "0.1.0"), .orderedDescending)
        XCTAssertEqual(AppUpdater.compareVersions("0.1.0", "v0.1.0"), .orderedSame)
        XCTAssertEqual(AppUpdater.compareVersions("v0.1", "0.1.0"), .orderedSame)
        XCTAssertEqual(AppUpdater.compareVersions("v0.1", "0.1.1"), .orderedAscending)
        XCTAssertEqual(AppUpdater.compareVersions("0.10.0", "0.9.0"), .orderedDescending)
        XCTAssertNil(AppUpdater.compareVersions("beta", "0.1.0"))
        XCTAssertNil(AppUpdater.compareVersions("", "0.1.0"))
    }

    func testReleaseIsNewerOnlyWhenParseableAndGreater() {
        let release = AppRelease(tagName: "v0.2.0", url: nil, dmgURL: nil, checksumsURL: nil)
        XCTAssertTrue(release.isNewerThan("0.1.0"))
        XCTAssertFalse(release.isNewerThan("0.2.0"))
        // 开发构建（无 bundle 版本）不触发更新提示。
        XCTAssertFalse(release.isNewerThan(nil))
    }

    func testParseReleaseExtractsDMGAndChecksumAssets() throws {
        let json = """
        {
          "tag_name": "v0.2.0",
          "html_url": "https://github.com/Ash-one/MacAIConsole/releases/tag/v0.2.0",
          "assets": [
            {"name": "macai-v0.2.0-darwin-arm64.tar.gz",
             "browser_download_url": "https://example.com/macai.tar.gz"},
            {"name": "MacAIConsole.dmg",
             "browser_download_url": "https://example.com/MacAIConsole.dmg"},
            {"name": "checksums.txt",
             "browser_download_url": "https://example.com/checksums.txt"}
          ]
        }
        """
        let release = try XCTUnwrap(AppUpdater.parseRelease(Data(json.utf8)))
        XCTAssertEqual(release.tagName, "v0.2.0")
        XCTAssertEqual(release.dmgURL?.absoluteString, "https://example.com/MacAIConsole.dmg")
        XCTAssertEqual(release.checksumsURL?.absoluteString, "https://example.com/checksums.txt")
    }

    func testParseReleaseRejectsPayloadWithoutDMG() throws {
        let json = """
        {"tag_name": "v0.2.0", "assets": []}
        """
        let release = try XCTUnwrap(AppUpdater.parseRelease(Data(json.utf8)))
        XCTAssertNil(release.dmgURL)
        XCTAssertNil(release.checksumsURL)
    }

    func testChecksumManifestParsing() {
        let manifest = """
        1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef  macai-v0.2.0-darwin-arm64.tar.gz
        fdec1a2b3c4d5e6f00112233445566778899aabbccddeeff0011223344556677  MacAIConsole.dmg
        """
        XCTAssertEqual(
            AppUpdater.checksum(in: manifest, for: "MacAIConsole.dmg"),
            "fdec1a2b3c4d5e6f00112233445566778899aabbccddeeff0011223344556677"
        )
        XCTAssertNil(AppUpdater.checksum(in: manifest, for: "missing.dmg"))
        // 非 64 位十六进制的行视为无效，不返回。
        XCTAssertNil(AppUpdater.checksum(in: "deadbeef  MacAIConsole.dmg", for: "MacAIConsole.dmg"))
    }

    func testVerifyRejectsDigestMismatch() throws {
        let temporary = FileManager.default.temporaryDirectory
            .appending(path: "macai-update-test-\(UUID().uuidString).dmg")
        let data = Data("payload".utf8)
        try data.write(to: temporary)
        defer { try? FileManager.default.removeItem(at: temporary) }

        let actual = try AppUpdater.sha256Hex(ofFileAt: temporary)
        XCTAssertTrue(try AppUpdater.verify(dmgFile: temporary, manifest: "\(actual)  MacAIConsole.dmg"))
        XCTAssertFalse(
            try AppUpdater.verify(
                dmgFile: temporary,
                manifest: "\(String(repeating: "0", count: 64))  MacAIConsole.dmg"
            )
        )
    }
}
