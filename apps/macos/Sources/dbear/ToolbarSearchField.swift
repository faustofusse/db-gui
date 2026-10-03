import AppKit
import DBKit
import SwiftUI

/// Native NSSearchField hosted as a regular toolbar item, so items can sit after it
/// (SwiftUI's `.searchable` always pins the field to the trailing edge).
///
/// Two modes: live search (`onSubmit == nil`, `text` filters as you type), or a submitted query
/// (`onSubmit` runs on Return and when the field is cleared with ✕), used for table filters.
/// Esc leaves the field (after closing the completion list, if open) without clearing it.
struct ToolbarSearchField: NSViewRepresentable {
    @Binding var text: String
    var prompt: String = "Search"
    var help: String? = nil
    /// Monospaced text, for SQL.
    var monospaced = false
    var onSubmit: ((String) -> Void)? = nil
    /// SQL completion for the typed text (caret at a UTF-16 offset), with the script editor's popup
    /// and keys. `nil` = no completion.
    var complete: ((_ text: String, _ location: Int) -> Completions?)? = nil
    /// Bump to move keyboard focus into the field (⌘L / ⌘F).
    var focusRequest: Int = 0
    /// Esc: before focus leaves, e.g. to drop text typed but not applied.
    var onCancel: (() -> Void)? = nil
    /// Keyboard focus entered (true) or left (false) the field.
    var onFocusChange: ((Bool) -> Void)? = nil

    func makeNSView(context: Context) -> NSSearchField {
        let field = FocusReportingSearchField()
        field.onFocusChange = { [weak coordinator = context.coordinator] focused in
            coordinator?.parent.onFocusChange?(focused)
        }
        field.delegate = context.coordinator
        field.target = context.coordinator
        field.action = #selector(Coordinator.submitted(_:))
        return field
    }

    func updateNSView(_ field: NSSearchField, context: Context) {
        context.coordinator.parent = self
        if field.stringValue != text { field.stringValue = text }
        if field.placeholderString != prompt { field.placeholderString = prompt }
        if field.toolTip != help { field.toolTip = help }
        let submits = onSubmit != nil
        if field.sendsWholeSearchString != submits {
            // Submitted queries only fire on Return (or clearing), never per keystroke.
            field.sendsWholeSearchString = submits
            field.sendsSearchStringImmediately = !submits
        }
        let font = monospaced
            ? NSFont.monospacedSystemFont(ofSize: NSFont.systemFontSize, weight: .regular)
            : NSFont.systemFont(ofSize: NSFont.systemFontSize)
        if field.font != font { field.font = font }
        if focusRequest != context.coordinator.lastFocusRequest {
            context.coordinator.lastFocusRequest = focusRequest
            DispatchQueue.main.async { field.window?.makeFirstResponder(field) }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    /// Reports focus as it happens: delegate callbacks only start once the user types.
    final class FocusReportingSearchField: NSSearchField {
        var onFocusChange: ((Bool) -> Void)?

        override func becomeFirstResponder() -> Bool {
            let became = super.becomeFirstResponder()
            if became { onFocusChange?(true) }
            return became
        }

        /// The field editor resigned (click elsewhere, Tab, window closing…).
        override func textDidEndEditing(_ notification: Notification) {
            super.textDidEndEditing(notification)
            onFocusChange?(false)
        }
    }

    @MainActor
    final class Coordinator: NSObject, NSSearchFieldDelegate {
        var parent: ToolbarSearchField
        var lastFocusRequest: Int

        /// The same completion behaviour as the script editor, on the field's field editor.
        let completion = CompletionSession()
        /// Text before the latest change, to tell typing from deleting.
        private var previousText = ""

        init(parent: ToolbarSearchField) {
            self.parent = parent
            self.lastFocusRequest = parent.focusRequest
            super.init()
            completion.complete = { [weak self] text, location in self?.parent.complete?(text, location) }
        }

        func controlTextDidBeginEditing(_ note: Notification) {
            guard let field = note.object as? NSSearchField else { return }
            completion.textView = field.currentEditor() as? NSTextView
            previousText = field.stringValue
        }

        func controlTextDidEndEditing(_ note: Notification) {
            completion.dismiss()
        }

        /// While the popup is open, up/down move, Return/Tab accept (instead of applying the filter),
        /// Esc closes it. Otherwise Esc leaves the field instead of clearing it (NSSearchField's default).
        func control(_ control: NSControl, textView: NSTextView, doCommandBy selector: Selector) -> Bool {
            if completion.handle(command: selector) { return true }
            guard selector == #selector(NSResponder.cancelOperation(_:)) else { return false }
            parent.onCancel?()
            control.window?.makeFirstResponder(nil)
            return true
        }

        /// Return, or ✕ clearing the field (and every keystroke in live mode).
        @objc func submitted(_ sender: NSSearchField) {
            parent.text = sender.stringValue
            parent.onSubmit?(sender.stringValue)
        }

        func controlTextDidChange(_ note: Notification) {
            guard let field = note.object as? NSSearchField else { return }
            parent.text = field.stringValue
            guard parent.complete != nil, let editor = field.currentEditor() as? NSTextView else { return }
            completion.textView = editor
            // What was just typed: the character before the caret, if the text grew.
            let text = field.stringValue as NSString
            let caret = editor.selectedRange().location
            let grew = text.length > (previousText as NSString).length
            previousText = field.stringValue
            let inserted = grew && caret > 0 && caret <= text.length ? text.substring(with: NSRange(location: caret - 1, length: 1)) : nil
            completion.textDidChange(inserted: inserted)
        }
    }
}
