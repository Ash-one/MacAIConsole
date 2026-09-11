import AppKit
import SwiftUI

/// SwiftUI 持有源码；AppKit 文本系统只负责原生编辑行为和显示属性。
struct RunnerScriptTextEditor: NSViewRepresentable {
    @Environment(\.isEnabled) private var isEnabled
    @Binding var text: String

    func makeCoordinator() -> Coordinator {
        Coordinator(text: $text)
    }

    func makeNSView(context: Context) -> NSScrollView {
        let scrollView = NSScrollView()
        scrollView.borderType = .noBorder
        scrollView.drawsBackground = true
        scrollView.backgroundColor = .textBackgroundColor
        scrollView.hasVerticalScroller = true
        scrollView.autohidesScrollers = true

        let textView = NSTextView(frame: .zero)
        textView.delegate = context.coordinator
        textView.isRichText = false
        textView.importsGraphics = false
        textView.allowsUndo = true
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.textContainerInset = NSSize(width: 10, height: 10)
        textView.textContainer?.lineFragmentPadding = 0
        textView.textContainer?.widthTracksTextView = true
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isContinuousSpellCheckingEnabled = false
        textView.setAccessibilityLabel("Runner Python 源码")
        scrollView.documentView = textView
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        guard let textView = scrollView.documentView as? NSTextView else { return }
        context.coordinator.update(binding: $text)
        context.coordinator.isUpdating = true
        if textView.string != text {
            let selection = textView.selectedRange()
            textView.string = text
            let location = min(selection.location, text.utf16.count)
            textView.setSelectedRange(NSRange(location: location, length: 0))
        }
        textView.isEditable = isEnabled
        Self.applyHighlighting(to: textView)
        context.coordinator.isUpdating = false
    }

    static func highlightedSource(_ source: String) -> NSAttributedString {
        let storage = NSTextStorage(string: source)
        applyHighlighting(to: storage)
        return NSAttributedString(attributedString: storage)
    }

    private static let font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
    private static let commentPattern = try! NSRegularExpression(pattern: #"(?m)#.*$"#)
    private static let stringPattern = try! NSRegularExpression(
        pattern: #"(?s)(\"\"\".*?\"\"\"|'''.*?''')|(\"(?:\\.|[^\"\\])*\"|'(?:\\.|[^'\\])*')"#
    )
    private static let numberPattern = try! NSRegularExpression(pattern: #"\b\d+(?:\.\d+)?\b"#)
    private static let keywordPattern = try! NSRegularExpression(
        pattern: #"\b(?:and|as|def|else|False|for|from|global|if|import|in|is|None|not|or|pass|raise|return|True|with|yield)\b"#
    )
    private static let functionPattern = try! NSRegularExpression(
        pattern: #"\bdef\s+([A-Za-z_][A-Za-z0-9_]*)"#
    )
    private static let metadataKeyPattern = try! NSRegularExpression(
        pattern: #"(?m)^#\s+([A-Za-z][A-Za-z0-9_.-]*)\s*="#
    )
    private static let metadataTablePattern = try! NSRegularExpression(
        pattern: #"(?m)^#\s+(\[\[?[A-Za-z0-9_.-]+\]?\])\s*$"#
    )

    private static var baseAttributes: [NSAttributedString.Key: Any] {
        [.font: font, .foregroundColor: NSColor.labelColor]
    }

    private static func applyHighlighting(to textView: NSTextView) {
        guard let storage = textView.textStorage else { return }
        applyHighlighting(to: storage)
        textView.typingAttributes = baseAttributes
    }

    private static func applyHighlighting(to storage: NSTextStorage) {
        let fullRange = NSRange(location: 0, length: storage.length)
        storage.beginEditing()
        storage.setAttributes(baseAttributes, range: fullRange)
        // ponytail: 模板语法用正则着色；出现真实解析错误需求时再换语法解析器。
        color(commentPattern, in: storage, range: fullRange, with: .secondaryLabelColor)
        color(stringPattern, in: storage, range: fullRange, with: .systemRed)
        color(numberPattern, in: storage, range: fullRange, with: .systemBlue)
        color(keywordPattern, in: storage, range: fullRange, with: .systemPurple)
        color(functionPattern, group: 1, in: storage, range: fullRange, with: .systemTeal)
        color(metadataKeyPattern, group: 1, in: storage, range: fullRange, with: .systemBlue)
        color(metadataTablePattern, group: 1, in: storage, range: fullRange, with: .systemPurple)
        storage.endEditing()
    }

    private static func color(
        _ expression: NSRegularExpression,
        group: Int = 0,
        in storage: NSTextStorage,
        range: NSRange,
        with color: NSColor
    ) {
        expression.enumerateMatches(in: storage.string, range: range) { match, _, _ in
            guard let match else { return }
            storage.addAttribute(.foregroundColor, value: color, range: match.range(at: group))
        }
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        @Binding private var text: String
        fileprivate var isUpdating = false

        init(text: Binding<String>) {
            _text = text
        }

        fileprivate func update(binding: Binding<String>) {
            _text = binding
        }

        func textDidChange(_ notification: Notification) {
            guard !isUpdating, let textView = notification.object as? NSTextView else { return }
            text = textView.string
            RunnerScriptTextEditor.applyHighlighting(to: textView)
        }
    }
}
