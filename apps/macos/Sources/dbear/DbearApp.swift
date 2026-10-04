import AppKit
import SwiftUI

@main
struct DbearApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model = AppModel()
    @State private var updater = Updater()

    init() {
        ThinScrollers.install()
    }

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environment(model)
                .frame(minWidth: 900, minHeight: 500)
        }
        .defaultSize(width: 1400, height: 880)
        .commands {
            AppCommands(model: model)
            UpdateCommands(updater: updater)
        }

        Settings { SettingsView(updater: updater) }
    }
}

/// Needed when launched via `swift run` (no .app bundle): makes it a regular foreground app.
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationWillFinishLaunching(_ notification: Notification) {

        // We draw our own tabs; drop the native window-tab menu items.
        NSWindow.allowsAutomaticWindowTabbing = false
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}
