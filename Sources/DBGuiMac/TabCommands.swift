import AppKit
import SwiftUI

/// Tab navigation in the Window menu, with Safari's shortcuts.
struct TabCommands: Commands {
    let model: AppModel

    var body: some Commands {
        // Safari-style: ⌘W closes the tab (or the window when no tabs are left), ⇧⌘W closes the window.
        CommandGroup(replacing: .saveItem) {
            Button(model.tabs.isEmpty ? "Close Window" : "Close Tab") {
                if let id = model.activeTabID {
                    model.close(id)
                } else {
                    NSApp.keyWindow?.performClose(nil)
                }
            }
            .keyboardShortcut("w", modifiers: .command)

            Button("Close Window") { NSApp.keyWindow?.performClose(nil) }
                .keyboardShortcut("w", modifiers: [.command, .shift])
        }

        CommandGroup(after: .textEditing) {
            Button("Find") { model.searchFocusRequest += 1 }
                .keyboardShortcut("f", modifiers: .command)
        }

        CommandGroup(before: .windowList) {
            Button("Show Previous Tab") { model.selectAdjacentTab(offset: -1) }
                .keyboardShortcut("[", modifiers: [.command, .shift])
                .disabled(model.tabs.count < 2)
            Button("Show Next Tab") { model.selectAdjacentTab(offset: 1) }
                .keyboardShortcut("]", modifiers: [.command, .shift])
                .disabled(model.tabs.count < 2)

            Menu("Select Tab") {
                ForEach(1...9, id: \.self) { number in
                    Button(number == 9 ? "Last Tab" : "Tab \(number)") {
                        model.selectTab(number: number)
                    }
                    .keyboardShortcut(KeyEquivalent(Character("\(number)")), modifiers: .command)
                    .disabled(number == 9 ? model.tabs.isEmpty : model.tabs.count < number)
                }
            }

            Divider()
        }
    }
}
