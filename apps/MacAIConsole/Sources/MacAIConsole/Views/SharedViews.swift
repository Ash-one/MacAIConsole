import SwiftUI

/// 状态徽标：绿=在线，橙=启动/停止中，红=离线。
struct StatusBadge: View {
    let phase: DaemonController.Phase

    var body: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(color)
                .frame(width: 8, height: 8)
            Text(label)
                .font(.callout.weight(.medium))
        }
        .foregroundStyle(color)
        .padding(.horizontal, 10)
        .padding(.vertical, 5)
        .background(Capsule().fill(color.opacity(0.14)))
        .animation(.snappy(duration: 0.3), value: phase)
    }

    private var color: Color {
        switch phase {
        case .online: .green
        case .starting, .stopping: .orange
        case .offline: .red
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

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 4) {
                Text(title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(value)
                    .font(.title3.weight(.semibold).monospacedDigit())
                    .contentTransition(.numericText())
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(2)
        }
    }
}

/// 错误横幅。提供 `onClose` 时显示关闭按钮。
struct ErrorBanner: View {
    let text: String
    var onClose: (() -> Void)?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
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
            }
        }
        .padding(10)
        .background(RoundedRectangle(cornerRadius: 8).fill(.red.opacity(0.10)))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(.red.opacity(0.35)))
    }
}

/// 模型类型图标。
struct ModelTypeIcon: View {
    let type: String

    var body: some View {
        Image(systemName: symbol)
            .font(.caption)
            .frame(width: 22, height: 22)
            .background(RoundedRectangle(cornerRadius: 6).fill(Color.accentColor.opacity(0.15)))
            .foregroundStyle(Color.accentColor)
    }

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
/// 停止类传红色、启动类传绿色。
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
                    isDisabled ? Color(nsColor: .disabledControlTextColor)
                        : isHovering ? activeTint : Color(nsColor: .secondaryLabelColor)
                )
                .background(
                    Circle()
                        .fill(Color.primary.opacity(isHovering && !isDisabled ? 0.08 : 0))
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
    }
}

/// 离线提示卡片。
struct OfflineHint: View {
    let onStart: () -> Void

    var body: some View {
        GroupBox {
            VStack(spacing: 10) {
                Image(systemName: "antenna.radiowaves.left.and.right.slash")
                    .font(.title2)
                    .foregroundStyle(.orange)
                Text("守护进程未运行")
                    .font(.headline)
                Text("模型管理需要 aiworkd 在线。也可以在「设置」中开启随应用自动启动。")
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                Button("启动 aiworkd", action: onStart)
                    .buttonStyle(.borderedProminent)
            }
            .frame(maxWidth: .infinity)
            .padding(12)
        }
    }
}