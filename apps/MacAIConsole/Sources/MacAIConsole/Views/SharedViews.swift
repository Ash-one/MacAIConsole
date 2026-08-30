import SwiftUI

// MARK: - 基础元件

/// 状态圆点：可选辉光与呼吸动画（尊重「减弱动态效果」）。
struct StatusDot: View {
    let color: Color
    var size: CGFloat = 8
    var glow: Bool = false
    var pulse: Bool = false

    @State private var isPulsing = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .shadow(color: glow ? color.opacity(0.55) : .clear, radius: size * 0.5)
            .opacity(isPulsing ? 0.4 : 1)
            .onAppear {
                guard pulse, !reduceMotion else { return }
                withAnimation(.easeInOut(duration: 1.1).repeatForever(autoreverses: true)) {
                    isPulsing = true
                }
            }
    }
}

/// 胶囊徽章：语义色传 `color`（背景 13% 透明度），不传则为中性灰。
/// 用于加速策略 tag、「独立进程」、任务状态药丸、计数等。
struct Chip: View {
    let text: String
    var color: Color?

    var body: some View {
        Text(text)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(foreground)
            .padding(.horizontal, 7)
            .padding(.vertical, 2.5)
            .background(Capsule().fill(backgroundFill))
            .overlay(Capsule().strokeBorder(borderFill))
    }

    private var foreground: Color {
        color ?? Color.primary.opacity(0.55)
    }

    private var backgroundFill: Color {
        color?.opacity(0.13) ?? Color.white.opacity(0.07)
    }

    private var borderFill: Color {
        color?.opacity(0.22) ?? Theme.hairline
    }
}

/// 发丝分隔线：卡片内部行与行之间的分隔。
struct HairlineDivider: View {
    var body: some View {
        Rectangle()
            .fill(Theme.hairline)
            .frame(height: 1)
    }
}

/// 区块卡片：标题行（图标 + 标题 + info 提示 + 副标题 + 尾部 accessory 槽）+ 内容，
/// 统一替代各页重复的 GroupBox + label 写法。
struct SectionCard<Content: View, Accessory: View>: View {
    let title: String
    var icon: String?
    var subtitle: String?
    /// 传入时在标题旁显示小 info 图标，悬停展示说明。
    var infoText: String?
    @ViewBuilder var accessory: Accessory
    @ViewBuilder var content: Content

    init(
        title: String,
        icon: String? = nil,
        subtitle: String? = nil,
        infoText: String? = nil,
        @ViewBuilder accessory: () -> Accessory = { EmptyView() },
        @ViewBuilder content: () -> Content
    ) {
        self.title = title
        self.icon = icon
        self.subtitle = subtitle
        self.infoText = infoText
        self.accessory = accessory()
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            content
        }
        .padding(Theme.Space.cardPadding)
        .frame(maxWidth: .infinity, alignment: .leading)
        .themedCard()
    }

    private var header: some View {
        HStack(spacing: 8) {
            if let icon {
                Image(systemName: icon)
                    .font(.footnote.weight(.semibold))
                    .foregroundStyle(Theme.accent)
            }
            Text(title)
                .font(.callout.weight(.semibold))
            if let infoText {
                Image(systemName: "info.circle")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .padding(3)
                    .contentShape(Rectangle())
                    .help(infoText)
            }
            if let subtitle {
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            accessory
        }
    }
}

// MARK: - 行为修饰

private struct HoverableRowModifier: ViewModifier {
    var cornerRadius: CGFloat

    @State private var isHovering = false

    func body(content: Content) -> some View {
        content
            .contentShape(Rectangle())
            .background(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .fill(isHovering ? Theme.surfaceHover : Color.clear)
            )
            .onHover { isHovering = $0 }
            .animation(.snappy(duration: 0.15), value: isHovering)
    }
}

extension View {
    /// 列表行悬停高亮：统一各页行 hover 的背景、命中区域与动画。
    func hoverableRow(cornerRadius: CGFloat = Theme.Radius.row) -> some View {
        modifier(HoverableRowModifier(cornerRadius: cornerRadius))
    }
}

// MARK: - 按钮

/// 品牌渐变主按钮：替代 `.borderedProminent`，深色文字压在 emerald→mint 渐变上。
/// 通过 `controlSize` 适配大小（行内小按钮用 `.small`）。
struct ProminentButtonStyle: ButtonStyle {
    @Environment(\.isEnabled) private var isEnabled
    @Environment(\.controlSize) private var controlSize

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(font)
            .padding(.horizontal, horizontalPadding)
            .padding(.vertical, verticalPadding)
            .foregroundStyle(.black.opacity(0.85))
            .background(
                Theme.accentGradient,
                in: RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                    .strokeBorder(.white.opacity(0.22))
            )
            .opacity(opacity(isPressed: configuration.isPressed))
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(.snappy(duration: 0.12), value: configuration.isPressed)
    }

    private var isCompact: Bool { controlSize == .small || controlSize == .mini }

    private var font: Font { isCompact ? .caption.weight(.semibold) : .callout.weight(.semibold) }
    private var horizontalPadding: CGFloat { isCompact ? 10 : 14 }
    private var verticalPadding: CGFloat { isCompact ? 3.5 : 6 }

    private func opacity(isPressed: Bool) -> Double {
        if !isEnabled { return 0.35 }
        return isPressed ? 0.75 : 1
    }
}

// MARK: - 业务组件

/// 状态徽标：emerald=在线，amber=启动/停止中，red=离线。
struct StatusBadge: View {
    let phase: DaemonController.Phase

    var body: some View {
        HStack(spacing: 7) {
            StatusDot(color: color, glow: true, pulse: phase != .offline)
            Text(label)
                .font(.callout.weight(.semibold))
        }
        .foregroundStyle(color)
        .padding(.horizontal, 12)
        .padding(.vertical, 6)
        .background(Capsule().fill(color.opacity(0.13)))
        .overlay(Capsule().strokeBorder(color.opacity(0.25)))
        .animation(.snappy(duration: 0.3), value: phase)
    }

    private var color: Color {
        switch phase {
        case .online: Theme.success
        case .starting, .stopping: Theme.warning
        case .offline: Theme.danger
        }
    }

    private var label: String {
        switch phase {
        case .online: "在线"
        case .starting: "启动中"
        case .stopping: "停止中"
        case .offline: "离线"
        }
    }
}

/// 状态统计卡。
struct StatCard: View {
    let title: String
    let value: String
    var icon: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(spacing: 5) {
                if let icon {
                    Image(systemName: icon)
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
                Text(title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Text(value)
                .font(.title3.weight(.semibold).monospacedDigit())
                .contentTransition(.numericText())
                .lineLimit(1)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(12)
        .themedCard(cornerRadius: 10)
    }
}

/// 错误横幅。提供 `onClose` 时显示关闭按钮。
struct ErrorBanner: View {
    let text: String
    var onClose: (() -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(Theme.danger)
            Text(text)
                .font(.callout)
            Spacer(minLength: 0)
            if let onClose {
                Button {
                    onClose()
                } label: {
                    Image(systemName: "xmark")
                        .font(.caption.weight(.semibold))
                }
                .buttonStyle(.borderless)
                .foregroundStyle(.secondary)
                .help("关闭提示")
                .accessibilityLabel("关闭提示")
            }
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: Theme.Radius.card, style: .continuous).fill(Theme.danger.opacity(0.10)))
        .overlay(RoundedRectangle(cornerRadius: Theme.Radius.card, style: .continuous).strokeBorder(Theme.danger.opacity(0.35)))
    }
}

/// 模型类型图标：按类型着色（LLM=emerald、STT=sky、TTS=violet）。
struct ModelTypeIcon: View {
    let type: String

    var body: some View {
        Image(systemName: symbol)
            .font(.caption.weight(.semibold))
            .frame(width: 24, height: 24)
            .background(
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .fill(color.opacity(0.16))
            )
            .foregroundStyle(color)
    }

    private var color: Color { Theme.typeColor(type) }

    private var symbol: String {
        switch type {
        case "llm": "brain"
        case "stt": "waveform"
        case "tts": "speaker.wave.2"
        default: "cube"
        }
    }
}

/// 列表行幽灵操作按钮：常态是安静的次级色圆形图标，悬停时浮现淡圆底并转为语义色。
/// 停止类传 danger 红、启动类传 success 绿。
struct GhostActionButton: View {
    let systemImage: String
    let help: String
    let activeTint: Color
    var isDisabled: Bool = false
    let action: () -> Void

    @State private var isHovering = false

    var body: some View {
        Button(action: action) {
            Image(systemName: systemImage)
                .font(.title3)
                .symbolVariant(.fill)
                .foregroundStyle(
                    isDisabled ? Color.primary.opacity(0.25)
                        : isHovering ? activeTint : Color.primary.opacity(0.5)
                )
                .background(
                    Circle()
                        .fill(Color.white.opacity(isHovering && !isDisabled ? 0.09 : 0))
                        .frame(width: 26, height: 26)
                )
                .opacity(isDisabled ? 0.5 : 1)
        }
        .buttonStyle(.plain)
        .controlSize(.small)
        .disabled(isDisabled)
        .onHover { isHovering = $0 }
        .animation(.snappy(duration: 0.15), value: isHovering)
        .help(help)
        .accessibilityLabel(help)
    }
}

/// 卡片内空状态占位。
struct EmptyHint: View {
    let text: String
    var systemImage: String = "tray"

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: systemImage)
                .font(.title2)
                .foregroundStyle(.quaternary)
            Text(text)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, minHeight: 52)
        .padding(.vertical, 6)
    }
}

/// 离线提示卡片。
struct OfflineHint: View {
    let onStart: () -> Void

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "antenna.radiowaves.left.and.right.slash")
                .font(.system(size: 28))
                .foregroundStyle(Theme.warning)
            Text("守护进程未运行")
                .font(.headline)
            Text("此页面需要 aiworkd 在线。也可以在「设置」中开启随应用自动启动。")
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
            Button("启动 aiworkd", action: onStart)
                .buttonStyle(ProminentButtonStyle())
                .padding(.top, 2)
        }
        .frame(maxWidth: .infinity)
        .padding(20)
        .themedCard()
    }
}
