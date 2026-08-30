import Foundation

enum ProxyMode: String, CaseIterable, Identifiable {
    case system
    case disabled
    case manual

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: "系统代理"
        case .disabled: "不使用代理"
        case .manual: "手动设置"
        }
    }
}

enum AppSettings {
    static let autoStartKey = "autoStartDaemon"
    static let memoryBudgetKey = "memoryBudget"
    static let logLevelKey = "logLevel"
    static let qwen3ASR06BEnabledKey = "qwen3ASR06BEnabled"
    static let proxyModeKey = "proxyMode"
    static let httpProxyKey = "httpProxy"
    static let httpsProxyKey = "httpsProxy"

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

    static var proxyMode: ProxyMode {
        proxyMode(in: .standard)
    }

    static var httpProxy: String? {
        normalizedProxyURL(UserDefaults.standard.string(forKey: httpProxyKey) ?? "")
    }

    static var httpsProxy: String? {
        normalizedProxyURL(UserDefaults.standard.string(forKey: httpsProxyKey) ?? "")
    }

    static func qwen3ASR06BEnabled(in defaults: UserDefaults) -> Bool {
        defaults.object(forKey: qwen3ASR06BEnabledKey) as? Bool ?? false
    }

    static func proxyMode(in defaults: UserDefaults) -> ProxyMode {
        let rawValue = defaults.string(forKey: proxyModeKey) ?? ProxyMode.system.rawValue
        return ProxyMode(rawValue: rawValue) ?? .system
    }

    static func normalizedProxyURL(_ text: String) -> String? {
        var value = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return nil }
        if !value.contains("://") {
            value = "http://\(value)"
        }
        guard let components = URLComponents(string: value),
              ["http", "https"].contains(components.scheme?.lowercased()),
              components.host?.isEmpty == false else { return nil }
        return components.string
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