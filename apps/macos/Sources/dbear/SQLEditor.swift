import AppKit
import DBKit
import SwiftUI

/// Plain-text SQL editor (NSTextView) with tree-sitter highlighting from the Rust core.
struct SQLEditor: NSViewRepresentable {
    @Binding var text: String
    /// Make the editor first responder with the caret at the end once it's in a window.
    var focusOnAppear = false

    func makeCoordinator() -> Coordinator { Coordinator(text: $text) }

    func makeNSView(context: Context) -> NSScrollView {
        let scrollView = NSTextView.scrollableTextView()
        scrollView.drawsBackground = false
        scrollView.hasHorizontalScroller = false

        let textView = scrollView.documentView as! NSTextView
        textView.delegate = context.coordinator
        textView.drawsBackground = false
        textView.isRichText = false
        textView.importsGraphics = false
        textView.allowsUndo = true
        textView.usesFindBar = true
        textView.isIncrementalSearchingEnabled = true
        textView.smartInsertDeleteEnabled = false
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.isContinuousSpellCheckingEnabled = false
        textView.isGrammarCheckingEnabled = false
        textView.isAutomaticLinkDetectionEnabled = false
        textView.isAutomaticDataDetectionEnabled = false
        textView.textContainerInset = NSSize(width: 10, height: 8)
        textView.font = SQLTheme.font
        textView.typingAttributes = SQLTheme.baseAttributes

        textView.string = text
        context.coordinator.textView = textView
        context.coordinator.highlight()

        if focusOnAppear {
            // Not in a window yet; wait a run loop turn.
            DispatchQueue.main.async { [weak textView] in
                guard let textView, let window = textView.window else { return }
                window.makeFirstResponder(textView)
                textView.setSelectedRange(NSRange(location: (textView.string as NSString).length, length: 0))
            }
        }
        return scrollView
    }

    func updateNSView(_ scrollView: NSScrollView, context: Context) {
        context.coordinator.text = $text
        guard let textView = context.coordinator.textView, textView.string != text else { return }
        // External change (not typed here): replace and keep the caret in range.
        let caret = min(textView.selectedRange().location, (text as NSString).length)
        textView.string = text
        textView.setSelectedRange(NSRange(location: caret, length: 0))
        context.coordinator.highlight()
    }

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate {
        var text: Binding<String>
        weak var textView: NSTextView?
        private var generation = 0

        init(text: Binding<String>) { self.text = text }

        func textDidChange(_ notification: Notification) {
            guard let textView else { return }
            text.wrappedValue = textView.string
            highlight()
        }

        /// Small scripts are highlighted synchronously (no flash of unstyled text);
        /// huge ones off the main thread, applied only if the text hasn't changed meanwhile.
        func highlight() {
            guard let textView else { return }
            let source = textView.string
            generation += 1
            if source.utf16.count < 50_000 {
                apply(SQLSyntax.highlight(source), to: textView)
                return
            }
            let generation = generation
            Task.detached(priority: .userInitiated) {
                let spans = SQLSyntax.highlight(source)
                await MainActor.run { [weak self] in
                    guard let self, self.generation == generation, let textView = self.textView else { return }
                    self.apply(spans, to: textView)
                }
            }
        }

        private func apply(_ spans: [SyntaxSpan], to textView: NSTextView) {
            guard let storage = textView.textStorage else { return }
            let length = storage.length
            storage.beginEditing()
            storage.setAttributes(SQLTheme.baseAttributes, range: NSRange(location: 0, length: length))
            for span in spans where NSMaxRange(span.range) <= length {
                storage.addAttributes(SQLTheme.attributes(for: span.kind), range: span.range)
            }
            storage.endEditing()
        }
    }
}

/// Xcode-like palette (Default Light / Default Dark), resolved per appearance at draw time.
@MainActor
enum SQLTheme {
    static let font = NSFont.monospacedSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
    static let keywordFont = NSFont.monospacedSystemFont(ofSize: NSFont.systemFontSize, weight: .semibold)

    static let baseAttributes: [NSAttributedString.Key: Any] = [
        .font: font,
        .foregroundColor: NSColor.textColor,
    ]

    static func attributes(for kind: SyntaxKind) -> [NSAttributedString.Key: Any] {
        cache[kind] ?? [:]
    }

    private static let cache: [SyntaxKind: [NSAttributedString.Key: Any]] = {
        var result: [SyntaxKind: [NSAttributedString.Key: Any]] = [:]
        for kind in SyntaxKind.allCases {
            var attrs: [NSAttributedString.Key: Any] = [:]
            if let color = color(for: kind) { attrs[.foregroundColor] = color }
            if kind == .keyword || kind == .constant { attrs[.font] = keywordFont }
            result[kind] = attrs
        }
        return result
    }()

    private static func color(for kind: SyntaxKind) -> NSColor? {
        switch kind {
        case .keyword, .constant: dynamic(light: 0x9B2393, dark: 0xFC5FA3)
        case .type: dynamic(light: 0x0B4F79, dark: 0x5DD8FF)
        case .object: dynamic(light: 0x1C464A, dark: 0x9EF1DD)
        case .function: dynamic(light: 0x326D74, dark: 0x67B7A4)
        case .string: dynamic(light: 0xC41A16, dark: 0xFC6A5D)
        case .number: dynamic(light: 0x1C00CF, dark: 0xD0BF69)
        case .comment: dynamic(light: 0x5D6C79, dark: 0x7F8C98)
        case .parameter: dynamic(light: 0x643820, dark: 0xFD8F3F)
        case .variable: dynamic(light: 0x3E8087, dark: 0x67B7A4)
        case .field, .operator, .punctuation: nil
        }
    }

    private static func dynamic(light: UInt32, dark: UInt32) -> NSColor {
        NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return rgb(isDark ? dark : light)
        }
    }

    private static func rgb(_ hex: UInt32) -> NSColor {
        NSColor(srgbRed: CGFloat((hex >> 16) & 0xFF) / 255,
                green: CGFloat((hex >> 8) & 0xFF) / 255,
                blue: CGFloat(hex & 0xFF) / 255,
                alpha: 1)
    }
}
