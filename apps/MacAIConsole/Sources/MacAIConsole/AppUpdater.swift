import CryptoKit
import Foundation
import Observation

/// 应用内更新：检查 GitHub Releases、校验并替换当前 .app bundle。
///
/// 更新只针对 GUI 自身的 bundle；daemon 由新 bundle 首次启动时按设置重新拉起。
enum AppUpdater {
    static let repository = "Ash-one/MacAIConsole"
    static let dmgAssetName = "MacAIConsole.dmg"
    static let checksumsAssetName = "checksums.txt"
    static let releasesAPI = URL(string: "https://api.github.com/repos/\(repository)/releases/latest")!

    /// 当前 bundle 的展示版本（CFBundleShortVersionString），无 bundle 时为 nil。
    static var currentVersion: String? {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
    }

    /// 仅当运行在已安装的 .app bundle 内时更新才有意义。
    static var canInstall: Bool {
        Bundle.main.bundlePath.hasSuffix(".app")
    }

    /// 数值化 semver 段比较，容忍 "v" 前缀与缺段（缺段按 0）。
    /// 非数字段直接返回 nil，由调用方决定降级行为。
    static func compareVersions(_ lhs: String, _ rhs: String) -> ComparisonResult? {
        let left = normalizedVersionSegments(lhs)
        let right = normalizedVersionSegments(rhs)
        guard let left, let right else { return nil }
        for index in 0..<max(left.count, right.count) {
            let l = index < left.count ? left[index] : 0
            let r = index < right.count ? right[index] : 0
            if l < r { return .orderedAscending }
            if l > r { return .orderedDescending }
        }
        return .orderedSame
    }

    private static func normalizedVersionSegments(_ version: String) -> [Int]? {
        var value = version.trimmingCharacters(in: .whitespaces)
        if value.hasPrefix("v") || value.hasPrefix("V") {
            value.removeFirst()
        }
        guard !value.isEmpty else { return nil }
        var segments: [Int] = []
        for part in value.split(separator: ".") {
            guard let number = Int(part) else { return nil }
            segments.append(number)
        }
        return segments
    }

    static func parseRelease(_ data: Data) -> AppRelease? {
        try? JSONDecoder().decode(GitHubRelease.self, from: data).appRelease
    }

    /// 从 checksums.txt（`<sha256>  <文件名>` 行）提取目标文件摘要。
    static func checksum(in manifest: String, for fileName: String) -> String? {
        for line in manifest.split(whereSeparator: \.isNewline) {
            let parts = line.split(whereSeparator: \.isWhitespace)
            guard parts.count == 2, parts[1] == fileName else { continue }
            let digest = parts[0].lowercased()
            return digest.count == 64 && digest.allSatisfy(\.isHexDigit) ? digest : nil
        }
        return nil
    }

    /// 流式计算文件摘要，避免把整个 DMG 读进内存。
    static func sha256Hex(ofFileAt fileURL: URL) throws -> String {
        var hasher = SHA256()
        let handle = try FileHandle(forReadingFrom: fileURL)
        defer { try? handle.close() }
        while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
            hasher.update(data: chunk)
        }
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }

    /// 校验下载的 DMG 是否与 checksums.txt 中记录的摘要一致。
    static func verify(dmgFile: URL, manifest: String) throws -> Bool {
        guard let expected = checksum(in: manifest, for: dmgAssetName) else { return false }
        return try sha256Hex(ofFileAt: dmgFile) == expected
    }

    static func fetchLatestRelease(session: URLSession = .shared) async throws -> AppRelease {
        var request = URLRequest(url: releasesAPI, timeoutInterval: 15)
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw AppUpdateError.feedUnavailable
        }
        guard let release = parseRelease(data) else {
            throw AppUpdateError.feedUnavailable
        }
        return release
    }
}

struct AppRelease: Equatable {
    var tagName: String
    var url: URL?
    var dmgURL: URL?
    var checksumsURL: URL?

    /// 与给定版本比较；任一版本无法解析时视为无更新。
    func isNewerThan(_ current: String?) -> Bool {
        guard let current,
              let result = AppUpdater.compareVersions(tagName, current) else { return false }
        return result == .orderedDescending
    }
}

enum AppUpdateError: LocalizedError {
    case feedUnavailable
    case assetMissing
    case checksumMismatch
    case installFailed(String)

    var errorDescription: String? {
        switch self {
        case .feedUnavailable: "无法获取更新信息，请检查网络。"
        case .assetMissing: "发布产物中缺少 DMG 或校验和文件。"
        case .checksumMismatch: "下载内容校验失败，已放弃安装。"
        case .installFailed(let detail): "安装新版本失败：\(detail)"
        }
    }
}

/// GitHub Releases API 的最小解码模型。
private struct GitHubRelease: Decodable {
    struct Asset: Decodable {
        var name: String
        var browserDownloadURL: String

        enum CodingKeys: String, CodingKey {
            case name
            case browserDownloadURL = "browser_download_url"
        }
    }

    var tagName: String
    var htmlURL: String?
    var assets: [Asset]

    enum CodingKeys: String, CodingKey {
        case tagName = "tag_name"
        case htmlURL = "html_url"
        case assets
    }

    var appRelease: AppRelease {
        AppRelease(
            tagName: tagName,
            url: htmlURL.flatMap(URL.init(string:)),
            dmgURL: assets.first { $0.name == AppUpdater.dmgAssetName }
                .flatMap { URL(string: $0.browserDownloadURL) },
            checksumsURL: assets.first { $0.name == AppUpdater.checksumsAssetName }
                .flatMap { URL(string: $0.browserDownloadURL) }
        )
    }
}

/// 驱动设置页更新 UI 的状态机：idle → checking → available/upToDate → downloading → installing。
@MainActor
@Observable
final class AppUpdateState {
    enum Status: Equatable {
        case idle
        case checking
        case upToDate
        case available(AppRelease)
        case downloading
        case installing
        case failed(String)
    }

    struct InstallHandlers {
        /// 安装前的收尾动作：停止 aiworkd 并等待其退出。
        var prepare: () async -> Void
        /// 安装失败后的恢复动作：把 daemon 重新拉起。
        var recovery: () -> Void
    }

    private(set) var status: Status = .idle
    private var installHandlers: InstallHandlers?

    func setInstallHandlers(
        prepare: @escaping () async -> Void,
        recovery: @escaping () -> Void
    ) {
        installHandlers = InstallHandlers(prepare: prepare, recovery: recovery)
    }

    func checkForUpdates(session: URLSession = .shared) async {
        guard AppUpdater.canInstall else { return }
        status = .checking
        do {
            let release = try await AppUpdater.fetchLatestRelease(session: session)
            if release.isNewerThan(AppUpdater.currentVersion) {
                status = .available(release)
            } else {
                status = .upToDate
            }
        } catch {
            status = .failed(error.localizedDescription)
        }
    }

    /// 启动时的静默检查：发现新版本才改变状态，其余情况回到 idle，不打扰用户。
    func checkSilently(session: URLSession = .shared) async {
        guard AppUpdater.canInstall else { return }
        do {
            let release = try await AppUpdater.fetchLatestRelease(session: session)
            if release.isNewerThan(AppUpdater.currentVersion) {
                status = .available(release)
            }
        } catch {
            // 静默检查失败不提示。
        }
    }

    func downloadAndInstall(session: URLSession = .shared) async {
        guard case .available(let release) = status,
              let dmgURL = release.dmgURL,
              let checksumsURL = release.checksumsURL else {
            status = .failed(AppUpdateError.assetMissing.localizedDescription)
            return
        }
        status = .downloading
        let workDir = FileManager.default.temporaryDirectory
            .appending(path: "macai-update-\(UUID().uuidString)")
        do {
            let dmgFile = workDir.appending(path: AppUpdater.dmgAssetName)
            let manifestFile = workDir.appending(path: AppUpdater.checksumsAssetName)
            try await download(dmgURL, to: dmgFile, session: session)
            try await download(checksumsURL, to: manifestFile, session: session)
            let manifest = try String(contentsOf: manifestFile, encoding: .utf8)
            guard try AppUpdater.verify(dmgFile: dmgFile, manifest: manifest) else {
                throw AppUpdateError.checksumMismatch
            }
            status = .installing
            await installHandlers?.prepare()
            try await install(dmgFile: dmgFile, workDir: workDir)
            // install() 成功后进程会被新 bundle 接替，不会执行到这里。
        } catch {
            status = .failed(error.localizedDescription)
            installHandlers?.recovery()
        }
        try? FileManager.default.removeItem(at: workDir)
    }

    /// 落盘下载，避免把整包 DMG 缓冲进内存。
    private func download(_ url: URL, to destination: URL, session: URLSession) async throws {
        let (temporaryFile, response) = try await session.download(from: url, delegate: nil)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw AppUpdateError.assetMissing
        }
        try FileManager.default.createDirectory(
            at: destination.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try? FileManager.default.removeItem(at: destination)
        try FileManager.default.moveItem(at: temporaryFile, to: destination)
    }

    /// 挂载 DMG、替换当前 bundle、重启应用。成功即清理现场并退出当前进程。
    private func install(dmgFile: URL, workDir: URL) async throws {
        let fileManager = FileManager.default
        let mountPoint = fileManager.temporaryDirectory
            .appending(path: "macai-update-\(UUID().uuidString)")
        let currentBundle = Bundle.main.bundleURL
        try fileManager.createDirectory(at: mountPoint, withIntermediateDirectories: true)

        try await runProcess("/usr/bin/hdiutil", [
            "attach", dmgFile.path, "-nobrowse", "-quiet", "-mountpoint", mountPoint.path,
        ])
        do {
            let entries = try fileManager.contentsOfDirectory(at: mountPoint, includingPropertiesForKeys: nil)
            guard let newBundle = entries.first(where: { $0.pathExtension == "app" }) else {
                throw AppUpdateError.assetMissing
            }

            let backup = fileManager.temporaryDirectory
                .appending(path: "MacAIConsole.old-\(UUID().uuidString)")
            try fileManager.moveItem(at: currentBundle, to: backup)
            do {
                try fileManager.copyItem(at: newBundle, to: currentBundle)
            } catch {
                try? fileManager.moveItem(at: backup, to: currentBundle)
                throw error
            }
            try? fileManager.removeItem(at: backup)
        } catch {
            try? await runProcess("/usr/bin/hdiutil", ["detach", mountPoint.path, "-quiet"])
            throw error
        }

        // exit(0) 不展开 defer，现场清理必须显式发生在退出之前。
        try? await runProcess("/usr/bin/hdiutil", ["detach", mountPoint.path, "-quiet"])
        try? fileManager.removeItem(at: workDir)

        try await runProcess("/usr/bin/open", [currentBundle.path])
        exit(0)
    }

    @discardableResult
    private func runProcess(_ path: String, _ arguments: [String]) async throws -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        try process.run()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            throw AppUpdateError.installFailed(
                "\(URL(fileURLWithPath: path).lastPathComponent) 退出码 \(process.terminationStatus)"
            )
        }
        return String(decoding: data, as: UTF8.self)
    }
}
