import Foundation

enum LogSource: String, CaseIterable, Identifiable {
    case gui
    case daemon

    var id: Self { self }

    var title: String {
        switch self {
        case .gui: "GUI"
        case .daemon: "Daemon"
        }
    }

    var fileURL: URL {
        switch self {
        case .gui: LogFiles.gui
        case .daemon: LogFiles.daemon
        }
    }
}

enum LogLevel: String, CaseIterable, Identifiable {
    case debug
    case info
    case warning
    case error

    var id: Self { self }

    var title: String {
        switch self {
        case .debug: "Debug"
        case .info: "Info"
        case .warning: "Warning"
        case .error: "Error"
        }
    }

    var priority: Int {
        switch self {
        case .debug: 0
        case .info: 1
        case .warning: 2
        case .error: 3
        }
    }

    func includes(_ entry: LogEntry) -> Bool {
        entry.level.priority >= priority
    }

    static func parse(from line: String) -> LogLevel {
        for token in line.split(whereSeparator: \.isWhitespace).prefix(5) {
            let level = token
                .trimmingCharacters(in: .punctuationCharacters)
                .uppercased()
            switch level {
            case "TRACE", "DEBUG": return .debug
            case "INFO": return .info
            case "WARN", "WARNING": return .warning
            case "ERROR": return .error
            default: continue
            }
        }
        return .info
    }
}

struct LogEntry: Identifiable, Equatable {
    var id: Int
    var level: LogLevel
    var text: String
}

struct RecentLogSnapshot: Equatable {
    var entries: [LogEntry]
    var modifiedAt: Date?

    var lineCount: Int { entries.count }
}

enum LogFiles {
    static let directory = FileManager.default.urls(
        for: .applicationSupportDirectory,
        in: .userDomainMask
    )[0]
        .appendingPathComponent("MacAIConsole", isDirectory: true)
        .appendingPathComponent("logs", isDirectory: true)

    static let gui = directory.appendingPathComponent("gui.log")
    static let daemon = directory.appendingPathComponent("aiworkd.log")

    private static let maximumBytes: UInt64 = 5 * 1024 * 1024

    /// 打开一个可追加的日志文件。超过 5 MB 时保留一份 `.1` 旧日志，避免无限增长。
    static func openForAppend(at url: URL) throws -> FileHandle {
        let fileManager = FileManager.default
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)

        if let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize,
           UInt64(size) >= maximumBytes {
            let archived = url.appendingPathExtension("1")
            try? fileManager.removeItem(at: archived)
            try fileManager.moveItem(at: url, to: archived)
        }

        if !fileManager.fileExists(atPath: url.path) {
            guard fileManager.createFile(atPath: url.path, contents: nil) else {
                throw CocoaError(.fileWriteUnknown)
            }
        }

        let handle = try FileHandle(forWritingTo: url)
        try handle.seekToEnd()
        return handle
    }
}

enum AppLogger {
    private static let queue = DispatchQueue(label: "org.macai.MacAIConsole.file-log")
    private static let formatter: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()

    static func info(_ message: String) {
        write(level: "INFO", message: message)
    }

    static func debug(_ message: String) {
        guard AppSettings.logLevel == .debug else { return }
        write(level: "DEBUG", message: message)
    }

    static func warning(_ message: String) {
        write(level: "WARN", message: message)
    }

    static func error(_ message: String) {
        write(level: "ERROR", message: message)
    }

    private static func write(level: String, message: String) {
        queue.async {
            let timestamp = formatter.string(from: Date())
            let line = "\(timestamp) \(level) \(message)\n"
            do {
                let handle = try LogFiles.openForAppend(at: LogFiles.gui)
                try handle.write(contentsOf: Data(line.utf8))
                try handle.close()
            } catch {
                // 文件日志自身失败时不递归记录，也不影响 GUI 主流程。
            }
        }
    }
}

enum RecentLogReader {
    static func read(
        source: LogSource,
        maxLines: Int = 500,
        maxBytes: UInt64 = 512 * 1024
    ) throws -> RecentLogSnapshot {
        try read(url: source.fileURL, maxLines: maxLines, maxBytes: maxBytes)
    }

    /// read 的后台版本：读文件与解析离开主线程，供日志页的轮询刷新调用。
    static func readInBackground(source: LogSource) async throws -> RecentLogSnapshot {
        try await Task.detached(priority: .userInitiated) {
            try read(source: source)
        }.value
    }

    static func read(
        url: URL,
        maxLines: Int = 500,
        maxBytes: UInt64 = 512 * 1024
    ) throws -> RecentLogSnapshot {
        let fileManager = FileManager.default
        guard fileManager.fileExists(atPath: url.path) else {
            return RecentLogSnapshot(entries: [], modifiedAt: nil)
        }

        let values = try url.resourceValues(forKeys: [.fileSizeKey, .contentModificationDateKey])
        let size = UInt64(values.fileSize ?? 0)
        let offset = size > maxBytes ? size - maxBytes : 0
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        try handle.seek(toOffset: offset)
        let data = try handle.readToEnd() ?? Data()

        var text = String(decoding: data, as: UTF8.self)
        if offset > 0, let firstNewline = text.firstIndex(of: "\n") {
            text.removeSubrange(text.startIndex...firstNewline)
        }
        text = stripANSI(from: text)

        var lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        if lines.last == "" { lines.removeLast() }
        let recent = Array(lines.suffix(max(maxLines, 1)))
        let entries = recent.enumerated().map { index, line in
            LogEntry(id: index, level: LogLevel.parse(from: line), text: line)
        }
        return RecentLogSnapshot(
            entries: entries,
            modifiedAt: values.contentModificationDate
        )
    }

    private static func stripANSI(from text: String) -> String {
        text.replacingOccurrences(
            of: "\u{001B}\\[[0-?]*[ -/]*[@-~]",
            with: "",
            options: .regularExpression
        )
    }
}
