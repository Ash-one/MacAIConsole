import Foundation

enum Format {
    private static let byteFormatter: ByteCountFormatter = {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .memory
        return formatter
    }()

    private static let relativeFormatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()

    static func bytes(_ value: UInt64?) -> String {
        guard let value else { return "—" }
        return byteFormatter.string(fromByteCount: Int64(value))
    }

    static func uptime(_ seconds: UInt64) -> String {
        let total = Int(seconds)
        let days = total / 86_400
        let hours = (total % 86_400) / 3_600
        let minutes = (total % 3_600) / 60
        let secs = total % 60
        if days > 0 { return "\(days) 天 \(hours) 小时" }
        if hours > 0 { return "\(hours) 小时 \(minutes) 分" }
        if minutes > 0 { return "\(minutes) 分 \(secs) 秒" }
        return "\(secs) 秒"
    }

    static func relativeTime(_ unixSeconds: UInt64?) -> String {
        guard let unixSeconds, unixSeconds > 0 else { return "—" }
        let date = Date(timeIntervalSince1970: TimeInterval(unixSeconds))
        return relativeFormatter.localizedString(for: date, relativeTo: Date())
    }
}