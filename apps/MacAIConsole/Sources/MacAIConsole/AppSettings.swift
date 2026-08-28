import Foundation

enum AppSettings {
    static let aiworkdPathKey = "aiworkdPath"
    static let autoStartKey = "autoStartDaemon"
    static let memoryBudgetKey = "memoryBudget"
    static let logLevelKey = "logLevel"
    static let qwen3ASR06BEnabledKey = "qwen3ASR06BEnabled"

    static var aiworkdPath: String? {
        let value = UserDefaults.standard.string(forKey: aiworkdPathKey)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return (value?.isEmpty == false) ? value : nil
    }

    static var autoStartDaemon: Bool {
        UserDefaults.standard.object(forKey: autoStartKey) as? Bool ?? true
    }

    /// 用户覆盖值。空字符串表示使用 daemon 的自动预算策略。
    static var memoryBudgetText: String {
        UserDefaults.standard.string(forKey: memoryBudgetKey) ?? ""
    }

    static var logLevel: LogLevel {
        let rawValue = UserDefaults.standard.string(forKey: logLevelKey) ?? LogLevel.info.rawValue
        return LogLevel(rawValue: rawValue) ?? .info
    }

    /// Qwen3-ASR is opt-in until its larger model footprint is explicitly accepted.
    static var qwen3ASR06BEnabled: Bool {
        qwen3ASR06BEnabled(in: .standard)
    }

    static func qwen3ASR06BEnabled(in defaults: UserDefaults) -> Bool {
        defaults.object(forKey: qwen3ASR06BEnabledKey) as? Bool ?? false
    }

    /// 支持 B / K / M / G / T 后缀；无后缀按字节解析。
    static func parseMemoryBudget(_ text: String) -> UInt64? {
        let raw = text.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !raw.isEmpty else { return nil }
        let suffixes: [(String, UInt64)] = [
            ("t", 1 << 40), ("tb", 1 << 40),
            ("g", 1 << 30), ("gb", 1 << 30),
            ("m", 1 << 20), ("mb", 1 << 20),
            ("k", 1 << 10), ("kb", 1 << 10),
            ("b", 1),
        ]
        for (suffix, multiplier) in suffixes where raw.hasSuffix(suffix) {
            let number = raw.dropLast(suffix.count).trimmingCharacters(in: .whitespaces)
            guard let value = Double(number), value > 0 else { return nil }
            let bytes = value * Double(multiplier)
            guard bytes <= Double(UInt64.max) else { return nil }
            return UInt64(bytes)
        }
        return UInt64(raw)
    }
}