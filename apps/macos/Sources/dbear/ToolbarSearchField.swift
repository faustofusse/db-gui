import AppKit
import SwiftUI

/// Native NSSearchField hosted as a regular toolbar item, so items can sit after it
/// (SwiftUI's `.searchable` always pins the field to the trailing edge).
struct ToolbarSearchField: NSViewRepresentable {
    @Binding var text: String
    var prompt: String = "Search"
    /// Bump to move keyboard focus into the field (⌘F).
    var focusRequest: Int = 0

    func makeNSView(context: Context) -> NSSearchField {
        let field = NSSearchField()
        field.placeholderString = prompt
        field.sendsSearchStringImmediately = true
        field.delegate = context.coordinator
        field.target = context.coordinator
        field.action = #selector(Coordinator.changed(_:))
        return field
    }

    func updateNSView(_ field: NSSearchField, context: Context) {
        context.coordinator.parent = self
        if field.stringValue != text { field.stringValue = text }
        if focusRequest != context.coordinator.lastFocusRequest {
            context.coordinator.lastFocusRequest = focusRequest
            DispatchQueue.main.async { field.window?.makeFirstResponder(field) }
        }
    }

    func makeCoordinator() -> Coordinator { Coordinator(parent: self) }

    @MainActor
    final class Coordinator: NSObject, NSSearchFieldDelegate {
        var parent: ToolbarSearchField
        var lastFocusRequest: Int

        init(parent: ToolbarSearchField) {
            self.parent = parent
            self.lastFocusRequest = parent.focusRequest
        }

        @objc func changed(_ sender: NSSearchField) {
            parent.text = sender.stringValue
        }

        func controlTextDidChange(_ note: Notification) {
            if let field = note.object as? NSSearchField { parent.text = field.stringValue }
        }
    }
}
