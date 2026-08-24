import Foundation

enum AppSettings {
    static let aiworkdPathKey = "aiworkdPath"
    static let autoStartKey = "autoStartDaemon"

    static var aiworkdPath: String? {
        let value = UserDefaults.standard.string(forKey: aiworkdPathKey)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return (value?.isEmpty == false) ? value : nil
    }

    static var autoStartDaemon: Bool {
        UserDefaults.standard.object(forKey: autoStartKey) as? Bool ?? true
    }
}