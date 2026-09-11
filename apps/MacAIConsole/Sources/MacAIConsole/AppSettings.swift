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

enum ModelDownloadSource: String, CaseIterable, Identifiable {
    case official
    case hfMirror = "hf-mirror"
    case custom

    var id: String { rawValue }

    var title: String {
        switch self {
        case .official: "Hugging Face 官方"
        case .hfMirror: "hf-mirror.com 镜像"
        case .custom: "自定义地址"
        }
    }
}

enum AppearanceMode: String, CaseIterable, Identifiable {
    case system
    case light
    case dark

    var id: String { rawValue }

    var title: String {
        switch self {
        case .system: "跟随系统"
        case .light: "明亮"
        case .dark: "暗黑"
        }
    }
}

enum AppSettings {
    static let autoStartKey = "autoStartDaemon"
    static let memoryBudgetKey = "memoryBudget"
    static let logLevelKey = "logLevel"
    static let appearanceKey = "appearance"
    static let proxyModeKey = "proxyMode"
    static let httpProxyKey = "httpProxy"
    static let httpsProxyKey = "httpsProxy"
    static let downloadSourceKey = "downloadSource"
    static let customDownloadEndpointKey = "customDownloadEndpoint"
    static let ignoredRecommendationIDsKey = "ignoredRecommendationIDs"
    static let ignoredRunnerIDsKey = "ignoredRunnerIDs"
    static let downloadEndpointEnvironmentKey = "AIWORKD_HF_ENDPOINT"
    static let officialDownloadEndpoint = "https://huggingface.co"
    static let hfMirrorDownloadEndpoint = "https://hf-mirror.com"

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

    static var appearance: AppearanceMode {
        appearance(in: .standard)
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

    static var downloadSource: ModelDownloadSource {
        downloadSource(in: .standard)
    }

    static var customDownloadEndpoint: String? {
        normalizedDownloadEndpoint(
            UserDefaults.standard.string(forKey: customDownloadEndpointKey) ?? ""
        )
    }

    static var ignoredRecommendationIDs: Set<String> {
        ignoredRecommendationIDs(in: .standard)
    }

    static var ignoredRunnerIDs: Set<String> {
        ignoredRunnerIDs(in: .standard)
    }

    static func proxyMode(in defaults: UserDefaults) -> ProxyMode {
        let rawValue = defaults.string(forKey: proxyModeKey) ?? ProxyMode.system.rawValue
        return ProxyMode(rawValue: rawValue) ?? .system
    }

    static func appearance(in defaults: UserDefaults) -> AppearanceMode {
        let rawValue = defaults.string(forKey: appearanceKey) ?? AppearanceMode.system.rawValue
        return AppearanceMode(rawValue: rawValue) ?? .system
    }

    static func downloadSource(in defaults: UserDefaults) -> ModelDownloadSource {
        let rawValue = defaults.string(forKey: downloadSourceKey) ?? ModelDownloadSource.official.rawValue
        return ModelDownloadSource(rawValue: rawValue) ?? .official
    }

    static func ignoredRecommendationIDs(in defaults: UserDefaults) -> Set<String> {
        Set(defaults.stringArray(forKey: ignoredRecommendationIDsKey) ?? [])
    }

    static func ignoreRecommendation(_ id: String, in defaults: UserDefaults = .standard) {
        var ids = ignoredRecommendationIDs(in: defaults)
        ids.insert(id)
        defaults.set(ids.sorted(), forKey: ignoredRecommendationIDsKey)
    }

    static func ignoredRunnerIDs(in defaults: UserDefaults) -> Set<String> {
        Set(defaults.stringArray(forKey: ignoredRunnerIDsKey) ?? [])
    }

    static func ignoreRunner(_ id: String, in defaults: UserDefaults = .standard) {
        var ids = ignoredRunnerIDs(in: defaults)
        ids.insert(id)
        defaults.set(ids.sorted(), forKey: ignoredRunnerIDsKey)
    }

    static func showAllRecommendations(in defaults: UserDefaults = .standard) {
        defaults.removeObject(forKey: ignoredRecommendationIDsKey)
    }

    static func showAllRunners(in defaults: UserDefaults = .standard) {
        defaults.removeObject(forKey: ignoredRunnerIDsKey)
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

    /// 规范化 Hugging Face 兼容源。允许省略 https://，但不接受凭据、查询参数或片段。
    static func normalizedDownloadEndpoint(_ text: String) -> String? {
        var value = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return nil }
        if !value.contains("://") {
            value = "https://\(value)"
        }
        guard let components = URLComponents(string: value),
              ["http", "https"].contains(components.scheme?.lowercased()),
              components.host?.isEmpty == false,
              components.user == nil,
              components.password == nil,
              components.query == nil,
              components.fragment == nil else { return nil }
        var normalized = components.string ?? value
        while normalized.hasSuffix("/") {
            normalized.removeLast()
        }
        return normalized.isEmpty ? nil : normalized
    }

    static func downloadEndpoint(
        for source: ModelDownloadSource,
        customEndpoint: String
    ) -> String? {
        switch source {
        case .official:
            officialDownloadEndpoint
        case .hfMirror:
            hfMirrorDownloadEndpoint
        case .custom:
            normalizedDownloadEndpoint(customEndpoint)
        }
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
