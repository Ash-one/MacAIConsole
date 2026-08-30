import SwiftUI

/// 设计令牌：暗色控制台视觉体系。
/// 颜色全部代码定义（无 Asset Catalog）；app 根部强制深色外观，
/// 因此这里直接给固定的暗色值，不随系统明暗模式变化。
enum Theme {
    // MARK: - 表面

    /// 页面底色：近黑的墨绿灰。
    static let background = Color(red: 0.055, green: 0.070, blue: 0.063)
    /// 卡片表面渐变：顶部略亮，制造轻微浮起感。
    static let surfaceTop = Color(red: 0.106, green: 0.126, blue: 0.117)
    static let surfaceBottom = Color(red: 0.082, green: 0.098, blue: 0.090)
    /// 行悬停高亮。
    static let surfaceHover = Color.white.opacity(0.06)
    /// 内嵌面板（日志正文、文本块、输入框）：比页面底色更深一档。
    static let inset = Color(red: 0.038, green: 0.049, blue: 0.044)

    /// 发丝描边：分隔线、卡片边框。
    static let hairline = Color.white.opacity(0.08)
    /// 更强的描边：输入框等需要明确边界的控件。
    static let hairlineStrong = Color.white.opacity(0.16)

    // MARK: - 品牌

    static let accent = Color(red: 0.204, green: 0.827, blue: 0.600)       // emerald
    static let accentBright = Color(red: 0.431, green: 0.906, blue: 0.718) // mint
    static let accentGradient = LinearGradient(
        colors: [accentBright, accent],
        startPoint: .top,
        endPoint: .bottom
    )

    // MARK: - 语义色

    static let success = accent
    static let warning = Color(red: 0.984, green: 0.749, blue: 0.141)
    static let danger = Color(red: 0.973, green: 0.443, blue: 0.443)
    static let info = Color(red: 0.220, green: 0.741, blue: 0.973)

    /// 模型类型色：LLM=emerald、STT=sky、TTS=violet，列表内一眼可辨。
    static func typeColor(_ type: String) -> Color {
        switch type {
        case "llm": accent
        case "stt": info
        case "tts": Color(red: 0.655, green: 0.545, blue: 0.980)
        default: .secondary
        }
    }

    // MARK: - 尺寸

    enum Space {
        static let page: CGFloat = 22
        static let sectionSpacing: CGFloat = 16
        static let cardPadding: CGFloat = 14
    }

    enum Radius {
        static let card: CGFloat = 12
        static let row: CGFloat = 8
        static let control: CGFloat = 7
    }
}

extension View {
    /// 控制台卡片外观：垂直渐变表面 + 发丝描边 + 轻投影。
    func themedCard(cornerRadius: CGFloat = Theme.Radius.card) -> some View {
        background(
            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                .fill(LinearGradient(
                    colors: [Theme.surfaceTop, Theme.surfaceBottom],
                    startPoint: .top,
                    endPoint: .bottom
                ))
        )
        .overlay(
            RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                .strokeBorder(Theme.hairline)
        )
        .shadow(color: .black.opacity(0.18), radius: 4, y: 2)
    }
}
