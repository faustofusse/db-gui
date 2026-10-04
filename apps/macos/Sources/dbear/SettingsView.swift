import SwiftUI

/// dbear ▸ Settings… (⌘,). Only updates for now.
struct SettingsView: View {
    @Bindable var updater: Updater

    var body: some View {
        Form {
            Section("Updates") {
                Toggle("Automatically check for updates", isOn: $updater.automaticallyChecks)
                Toggle("Automatically download and install updates", isOn: $updater.automaticallyInstalls)
                    .disabled(!updater.automaticallyChecks)
                LabeledContent("Version", value: updater.currentVersion)
                LabeledContent("Last checked", value: lastChecked)
                HStack {
                    Spacer()
                    Button("Check Now") { updater.checkForUpdates() }
                        .disabled(!updater.canCheckForUpdates)
                }
            }
            .disabled(!updater.isEnabled)
        }
        .formStyle(.grouped)
        .frame(width: 440)
        .fixedSize(horizontal: false, vertical: true)
    }

    private var lastChecked: String {
        guard updater.isEnabled else { return "Updates are off in this build" }
        guard let date = updater.lastCheck else { return "Never" }
        return date.formatted(.relative(presentation: .named))
    }
}
