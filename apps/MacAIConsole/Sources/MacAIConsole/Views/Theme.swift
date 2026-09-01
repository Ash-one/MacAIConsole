import AppKit
import SwiftUI

/// 设计令牌：保持 MacAI 品牌色的明暗自适应控制台视觉体系。
/// 颜色全部代码定义（无 Asset Catalog），基础表面通过 NSColor 动态提供器
/// 在明亮与暗黑外观之间切换。
enum Theme {
    // MARK: - 表面

    /// 页面底色：暗色为近黑的墨绿灰，亮色为带轻微绿色倾向的灰白。
    static let background = adaptive(
        light: NSColor(calibratedRed: 0.95, green: 0.965, blue: 0.955, alpha: 1),
        dark: NSColor(calibratedRed: 0.055, green: 0.070, blue: 0.063, alpha: 1)
    )
    /// 卡片表面渐变：顶部略亮，制造轻微浮起感。
    static let surfaceTop = adaptive(
        light: NSColor(calibratedRed: 0.985, green: 0.992, blue: 0.988, alpha: 1),
        dark: NSColor(calibratedRed: 0.106, green: 0.126, blue: 0.117, alpha: 1)
    )
    static let surfaceBottom = adaptive(
        light: NSColor(calibratedRed: 0.965, green: 0.975, blue: 0.970, alpha: 1),
        dark: NSColor(calibratedRed: 0.082, green: 0.098, blue: 0.090, alpha: 1)
    )
    /// 行悬停高亮。
    static let surfaceHover = adaptive(
        light: NSColor(calibratedWhite: 0, alpha: 0.04),
        dark: NSColor(calibratedWhite: 1, alpha: 0.06)
    )
    /// 内嵌面板（日志正文、文本块、输入框）：比页面底色更深一档。
    static let inset = adaptive(
        light: NSColor(calibratedRed: 0.90, green: 0.92, blue: 0.91, alpha: 1),
        dark: NSColor(calibratedRed: 0.038, green: 0.049, blue: 0.044, alpha: 1)
    )
    /// 悬浮层（信息气泡等临时浮层）：比卡片表面亮一档，配合投影表达悬浮。
    static let elevatedSurface = adaptive(
        light: NSColor(calibratedRed: 0.985, green: 0.992, blue: 0.988, alpha: 1),
        dark: NSColor(calibratedRed: 0.153, green: 0.173, blue: 0.162, alpha: 1)
    )

    /// 卡片投影：亮色模式以极轻的接触阴影保留边界，避免每个卡片都像悬浮层。
    static let cardShadow = adaptive(
        light: NSColor(calibratedWhite: 0, alpha: 0.05),
        dark: NSColor(calibratedWhite: 0, alpha: 0.14)
    )
    /// 信息气泡等真正的悬浮层使用更明确的投影。
    static let overlayShadow = adaptive(
        light: NSColor(calibratedWhite: 0, alpha: 0.14),
        dark: NSColor(calibratedWhite: 0, alpha: 0.28)
    )

    /// 发丝描边：分隔线、卡片边框。
    static let hairline = adaptive(
        light: NSColor(calibratedWhite: 0, alpha: 0.07),
        dark: NSColor(calibratedWhite: 1, alpha: 0.08)
    )
    /// 更强的描边：输入框等需要明确边界的控件。
    static let hairlineStrong = adaptive(
        light: NSColor(calibratedWhite: 0, alpha: 0.12),
        dark: NSColor(calibratedWhite: 1, alpha: 0.16)
    )

    // MARK: - 品牌

    static let accent = adaptive(
        light: NSColor(calibratedRed: 0.02, green: 0.53, blue: 0.34, alpha: 1),
        dark: NSColor(calibratedRed: 0.204, green: 0.827, blue: 0.600, alpha: 1)
    ) // emerald
    static let accentBright = adaptive(
        light: NSColor(calibratedRed: 0.08, green: 0.68, blue: 0.46, alpha: 1),
        dark: NSColor(calibratedRed: 0.431, green: 0.906, blue: 0.718, alpha: 1)
    ) // mint
    static let accentGradient = LinearGradient(
        colors: [accentBright, accent],
        startPoint: .top,
        endPoint: .bottom
    )

    // MARK: - 语义色

    static let success = accent
    static let warning = adaptive(
        light: NSColor(calibratedRed: 0.68, green: 0.39, blue: 0.02, alpha: 1),
        dark: NSColor(calibratedRed: 0.984, green: 0.749, blue: 0.141, alpha: 1)
    )
    static let danger = adaptive(
        light: NSColor(calibratedRed: 0.78, green: 0.18, blue: 0.18, alpha: 1),
        dark: NSColor(calibratedRed: 0.973, green: 0.443, blue: 0.443, alpha: 1)
    )
    static let info = adaptive(
        light: NSColor(calibratedRed: 0.02, green: 0.43, blue: 0.72, alpha: 1),
        dark: NSColor(calibratedRed: 0.220, green: 0.741, blue: 0.973, alpha: 1)
    )

    /// 模型类型色：LLM=emerald、STT=sky、TTS=violet，列表内一眼可辨。
    static func typeColor(_ type: String) -> Color {
        switch type {
        case "llm": accent
        case "stt": info
        case "tts": adaptive(
            light: NSColor(calibratedRed: 0.40, green: 0.28, blue: 0.78, alpha: 1),
            dark: NSColor(calibratedRed: 0.655, green: 0.545, blue: 0.980, alpha: 1)
        )
        default: .secondary
        }
    }

    private static func adaptive(light: NSColor, dark: NSColor) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
        })
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
        .shadow(color: Theme.cardShadow, radius: 2, y: 1)
    }
}
