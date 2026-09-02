import AppKit
import SwiftUI

/// AppKit 的文本系统负责日志的双轴滚动和选区；SwiftUI 仍持有日志条目这一唯一数据源。
struct LogConsoleView: NSViewRepresentable {
    @Environment(\.colorScheme) private var colorScheme

    let entries: [LogEntry]

    func makeCoordinator() -> Coordinator {
        Coordinator()
    }

    func makeNSView(context: Context) -> NSScrollView {
        let scrollView = NSScrollView()
        scrollView.borderType = .noBorder
        scrollView.drawsBackground = false
        scrollView.hasVerticalScroller = true
        scrollView.hasHorizontalScroller = true
        scrollView.autohidesScrollers = true

        let textView = NSTextView(frame: .zero)
        textView.isEditable = false
        textView.isSelectable = true
        textView.isRichText = true
        textView.allowsUndo = false
        textView.drawsBackground = false
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = true
        textView.textContainerInset = NSSize(width: 10, height: 10)
        textView.textContainer?.lineFragmentPadding = 0
        textView.textContainer?.widthTracksTextView = false
        textView.textContainer?.heightTracksTextView = false
        textView.textContainer?.containerSize = NSSize(
            width: CGFloat.greatestFiniteMagnitude,
            height: CGFloat.greatestFiniteMagnitude
        )
        textView.setAccessibilityLabel("日志内容")

        scrollView.documentView = textView
        context.coordinator.attach(to: scrollView)
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        guard let textView = scrollView.documentView as? NSTextView else { return }

        let coordinator = context.coordinator
        let oldOrigin = scrollView.contentView.bounds.origin
        let shouldFollowTail = coordinator.isAtBottom
        let entriesChanged = coordinator.entries != entries
        let appearanceChanged = coordinator.colorScheme != colorScheme

        coordinator.isUpdating = true
        if entriesChanged || appearanceChanged {
            textView.textStorage?.setAttributedString(Self.render(
                entries,
                appearance: textView.effectiveAppearance
            ))
            coordinator.entries = entries
            coordinator.colorScheme = colorScheme
        }
        Self.resizeDocument(textView, in: scrollView)

        if entriesChanged, shouldFollowTail {
            Self.scrollToBottom(scrollView)
        } else {
            Self.scroll(scrollView, to: oldOrigin)
        }
        coordinator.isUpdating = false
        coordinator.updateBottomState()
    }

    static func render(_ entries: [LogEntry], appearance: NSAppearance) -> NSAttributedString {
        let result = NSMutableAttributedString()
        let paragraphStyle = NSMutableParagraphStyle()
        paragraphStyle.paragraphSpacing = 2

        for (index, entry) in entries.enumerated() {
            let suffix = index == entries.indices.last ? "" : "\n"
            result.append(NSAttributedString(
                string: entry.text + suffix,
                attributes: [
                    .font: NSFont.monospacedSystemFont(ofSize: 11, weight: .regular),
                    .foregroundColor: color(for: entry.level, appearance: appearance),
                    .paragraphStyle: paragraphStyle,
                ]
            ))
        }
        return result
    }

    private static func color(for level: LogLevel, appearance: NSAppearance) -> NSColor {
        let color: NSColor
        let opacity: Double
        switch level {
        case .debug: (color, opacity) = (.labelColor, 0.35)
        case .info: (color, opacity) = (.labelColor, 0.85)
        case .warning: (color, opacity) = (.systemOrange, 1)
        case .error: (color, opacity) = (.systemRed, 1)
        }
        var resolvedColor = color
        appearance.performAsCurrentDrawingAppearance {
            resolvedColor = color.usingColorSpace(.deviceRGB) ?? color
        }
        return resolvedColor.withAlphaComponent(opacity)
    }

    private static func resizeDocument(_ textView: NSTextView, in scrollView: NSScrollView) {
        guard let layoutManager = textView.layoutManager,
              let textContainer = textView.textContainer else { return }

        layoutManager.ensureLayout(for: textContainer)
        let usedRect = layoutManager.usedRect(for: textContainer)
        let viewport = scrollView.contentSize
        let inset = textView.textContainerInset
        textView.setFrameSize(NSSize(
            width: max(viewport.width, ceil(usedRect.width + inset.width * 2)),
            height: max(viewport.height, ceil(usedRect.height + inset.height * 2))
        ))
    }

    private static func scrollToBottom(_ scrollView: NSScrollView) {
        let origin = scrollView.contentView.bounds.origin
        scroll(scrollView, to: NSPoint(x: origin.x, y: CGFloat.greatestFiniteMagnitude))
    }

    private static func scroll(_ scrollView: NSScrollView, to proposedOrigin: NSPoint) {
        guard let documentView = scrollView.documentView else { return }
        let viewport = scrollView.contentView.bounds.size
        let maximumX = max(documentView.frame.width - viewport.width, 0)
        let maximumY = max(documentView.frame.height - viewport.height, 0)
        let origin = NSPoint(
            x: min(max(proposedOrigin.x, 0), maximumX),
            y: min(max(proposedOrigin.y, 0), maximumY)
        )
        scrollView.contentView.scroll(to: origin)
        scrollView.reflectScrolledClipView(scrollView.contentView)
    }

    final class Coordinator: NSObject {
        fileprivate var entries: [LogEntry] = []
        fileprivate var colorScheme: ColorScheme?
        fileprivate var isAtBottom = true
        fileprivate var isUpdating = false
        private weak var scrollView: NSScrollView?
        private var boundsObserver: NSObjectProtocol?

        deinit {
            if let boundsObserver {
                NotificationCenter.default.removeObserver(boundsObserver)
            }
        }

        fileprivate func attach(to scrollView: NSScrollView) {
            self.scrollView = scrollView
            scrollView.contentView.postsBoundsChangedNotifications = true
            boundsObserver = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification,
                object: scrollView.contentView,
                queue: .main
            ) { [weak self] _ in
                self?.updateBottomState()
            }
        }

        fileprivate func updateBottomState() {
            guard !isUpdating,
                  let scrollView,
                  let documentView = scrollView.documentView else { return }
            let remaining = documentView.frame.height - scrollView.contentView.bounds.maxY
            isAtBottom = remaining <= 4
        }
    }
}
