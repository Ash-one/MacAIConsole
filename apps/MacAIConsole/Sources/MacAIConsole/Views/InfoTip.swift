import SwiftUI

/// 标题旁的信息气泡触发器。图标只上报自身位置，气泡由页面顶层的 presenter 绘制，
/// 避免 `Form` 的 Section 标题和相邻行遮挡跨出当前行的浮层。
struct InfoTip: View {
    let text: String
    var maxWidth: CGFloat? = nil

    @State private var isHovering = false
    @State private var isVisible = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        Image(systemName: "info.circle")
            .font(.caption)
            .foregroundStyle(isVisible ? Theme.accent : Color.primary.opacity(0.35))
            .padding(4)
            .contentShape(Rectangle())
            .anchorPreference(key: PresentationKey.self, value: .bounds) { anchor in
                guard isVisible else { return [] }
                return [Presentation(anchor: anchor, text: text, maxWidth: maxWidth)]
            }
            .onHover { isHovering = $0 }
            .task(id: isHovering) {
                if isHovering {
                    // 悬停片刻才触发，光标扫过标题行时任务会被取消，不会误弹。
                    try? await Task.sleep(for: .milliseconds(300))
                    guard !Task.isCancelled else { return }
                    withAnimation(popIn) { isVisible = true }
                } else {
                    withAnimation(.easeOut(duration: 0.12)) { isVisible = false }
                }
            }
            .accessibilityLabel(text)
    }

    private var popIn: Animation {
        reduceMotion ? .easeIn(duration: 0.12) : .snappy(duration: 0.28, extraBounce: 0.1)
    }
}

extension View {
    /// 在当前页面的最外层解析所有 `InfoTip` anchor，并在内容之上绘制唯一活跃气泡。
    func infoTipPresenter() -> some View {
        modifier(InfoTip.PresenterModifier())
    }
}

extension InfoTip {
    struct Layout {
        static let margin: CGFloat = 12
        static let gap: CGFloat = 8

        static func origin(
            bubbleSize: CGSize,
            target: CGRect,
            containerSize: CGSize
        ) -> CGPoint {
            let maximumX = max(margin, containerSize.width - margin - bubbleSize.width)
            let centeredX = target.midX - bubbleSize.width / 2
            let x = min(max(centeredX, margin), maximumX)
            return CGPoint(x: x, y: target.maxY + gap)
        }
    }

    fileprivate struct Presentation {
        let anchor: Anchor<CGRect>
        let text: String
        let maxWidth: CGFloat?
    }

    fileprivate struct PresentationKey: PreferenceKey {
        static var defaultValue: [Presentation] = []

        static func reduce(value: inout [Presentation], nextValue: () -> [Presentation]) {
            value.append(contentsOf: nextValue())
        }
    }

    fileprivate struct PresenterModifier: ViewModifier {
        @Environment(\.accessibilityReduceMotion) private var reduceMotion

        func body(content: Content) -> some View {
            content.overlayPreferenceValue(PresentationKey.self) { presentations in
                GeometryReader { proxy in
                    if let presentation = presentations.last {
                        let target = proxy[presentation.anchor]
                        let containerSize = proxy.size
                        Bubble(text: presentation.text, maxWidth: presentation.maxWidth)
                            .visualEffect { effect, bubbleProxy in
                                let origin = Layout.origin(
                                    bubbleSize: bubbleProxy.size,
                                    target: target,
                                    containerSize: containerSize
                                )
                                return effect.offset(x: origin.x, y: origin.y)
                            }
                            .transition(
                                reduceMotion
                                    ? .opacity
                                    : .scale(scale: 0.88, anchor: .top).combined(with: .opacity)
                            )
                    }
                }
                .allowsHitTesting(false)
            }
        }
    }

    fileprivate struct Bubble: View {
        let text: String
        let maxWidth: CGFloat?

        var body: some View {
            bubbleText
                .font(.caption)
                .foregroundStyle(Color.primary.opacity(0.85))
                .padding(.horizontal, 10)
                .padding(.vertical, 6)
                .background(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .fill(Theme.elevatedSurface)
                        .shadow(color: Theme.overlayShadow, radius: 6, y: 2)
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .strokeBorder(Theme.hairlineStrong)
                )
                .fixedSize()
        }

        @ViewBuilder
        private var bubbleText: some View {
            if let maxWidth {
                Text(text)
                    .frame(width: maxWidth, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
            } else {
                Text(text)
                    .fixedSize()
            }
        }
    }
}
