import AppKit
import Observation
import Sparkle
import SwiftUI

/// Auto-update through Sparkle. Defaults (Info.plist, see scripts/bundle-mac.sh): check daily,
/// download and verify in the background, install when the app quits. Scheduled checks never
/// show a window; a staged update only adds "Restart to Update" to the app menu. "Check for
/// Updates…" is the one path that shows Sparkle's dialog.
///
/// Inert unless running from a .app whose Info.plist has `SUFeedURL` and `SUPublicEDKey`
/// (`swift run`, tests, or a bundle built without a key).
@MainActor @Observable
final class Updater {
    /// An update Sparkle found during a scheduled check.
    enum Pending: Equatable {
        /// Downloaded and verified; installs on quit, or now with `installPendingUpdate()`.
        case readyToInstall(version: String)
        /// Couldn't be installed silently (e.g. needs an admin password); shown on request.
        case available(version: String)
    }

    private(set) var isEnabled = false
    private(set) var canCheckForUpdates = false
    private(set) var pending: Pending?
    private(set) var lastCheck: Date?

    var automaticallyChecks: Bool = false {
        didSet {
            guard let updater, updater.automaticallyChecksForUpdates != automaticallyChecks else { return }
            updater.automaticallyChecksForUpdates = automaticallyChecks
        }
    }

    var automaticallyInstalls: Bool = false {
        didSet {
            guard let updater, updater.automaticallyDownloadsUpdates != automaticallyInstalls else { return }
            updater.automaticallyDownloadsUpdates = automaticallyInstalls
        }
    }

    @ObservationIgnored private var controller: SPUStandardUpdaterController?
    @ObservationIgnored private let delegate = SparkleDelegate()
    @ObservationIgnored private var installNow: (() -> Void)?
    @ObservationIgnored private var observations: [NSKeyValueObservation] = []

    private var updater: SPUUpdater? { controller?.updater }

    var currentVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "dev"
    }

    init(bundle: Bundle = .main) {
        guard Self.isConfigured(bundle) else { return }
        delegate.owner = self
        let controller = SPUStandardUpdaterController(
            startingUpdater: true, updaterDelegate: delegate, userDriverDelegate: delegate)
        self.controller = controller
        isEnabled = true
        let updater = controller.updater
        automaticallyChecks = updater.automaticallyChecksForUpdates
        automaticallyInstalls = updater.automaticallyDownloadsUpdates
        lastCheck = updater.lastUpdateCheckDate
        observations = [
            updater.observe(\.canCheckForUpdates, options: [.initial, .new]) { [weak self] updater, _ in
                MainActor.assumeIsolated { self?.canCheckForUpdates = updater.canCheckForUpdates }
            },
            updater.observe(\.lastUpdateCheckDate, options: [.new]) { [weak self] updater, _ in
                MainActor.assumeIsolated { self?.lastCheck = updater.lastUpdateCheckDate }
            },
        ]
        runTestHookIfRequested(updater)
    }

    /// User-initiated check: Sparkle's standard dialog (also brings a pending update to focus).
    func checkForUpdates() {
        updater?.checkForUpdates()
    }

    /// Installs a staged update and relaunches, or shows an update that needs the user's attention.
    func installPendingUpdate() {
        switch pending {
        case .readyToInstall:
            if let installNow { installNow() } else { checkForUpdates() }
        case .available:
            checkForUpdates()
        case nil:
            break
        }
    }

    static func isConfigured(_ bundle: Bundle) -> Bool {
        func value(_ key: String) -> String {
            (bundle.object(forInfoDictionaryKey: key) as? String)?.trimmingCharacters(in: .whitespaces) ?? ""
        }
        return bundle.bundleURL.pathExtension == "app" && !value("SUFeedURL").isEmpty
            && !value("SUPublicEDKey").isEmpty
    }

    // MARK: Delegate callbacks (main thread)

    fileprivate func updateReadyOnQuit(version: String, install: @escaping () -> Void) {
        Self.testLog("ready \(version)")
        installNow = install
        pending = .readyToInstall(version: version)
        if Self.isTesting { Self.logAppMenuSoon() }
    }

    fileprivate func updateNeedsAttention(version: String) {
        if case .readyToInstall = pending { return }
        Self.testLog("available \(version)")
        pending = .available(version: version)
    }

    fileprivate func updateGotAttention() {
        if case .available = pending { pending = nil }
    }

    fileprivate static let isTesting = ProcessInfo.processInfo.environment["DBEAR_UPDATE_TEST"] == "1"

    /// Progress lines for `scripts/test-update.sh`, which reads the app's stdout.
    fileprivate static func testLog(_ message: String) {
        guard isTesting else { return }
        print("dbear-update-test: \(message)")
        fflush(stdout)
    }

    /// `scripts/test-update.sh`: check right away instead of waiting for the schedule. Only
    /// honored for a loopback feed, so a shipped build can't be poked into anything.
    private func runTestHookIfRequested(_ updater: SPUUpdater) {
        guard Self.isTesting, let host = updater.feedURL?.host, ["127.0.0.1", "localhost"].contains(host)
        else { return }
        Self.testLog("checking \(updater.feedURL?.absoluteString ?? "")")
        updater.checkForUpdatesInBackground()
        if ProcessInfo.processInfo.environment["DBEAR_UPDATE_TEST_SETTINGS"] == "1" {
            DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
                // Same as choosing dbear ▸ Settings… (for the test's screenshot).
                guard let menu = NSApp.mainMenu?.items.first?.submenu,
                      let index = menu.items.firstIndex(where: { $0.keyEquivalent == "," })
                else { return Self.testLog("no Settings… item") }
                menu.performActionForItem(at: index)
            }
        }
    }

    /// Logs the app menu's items once SwiftUI has rebuilt it, so the test can see "Restart to Update".
    private static func logAppMenuSoon() {
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
            // What the user sees when opening the menu (SwiftUI refreshes it on open).
            let menu = NSApp.mainMenu?.items.first?.submenu
            if let menu { menu.delegate?.menuNeedsUpdate?(menu) }
            let items = menu?.items.map(\.title) ?? []
            testLog("app menu: \(items.filter { !$0.isEmpty }.joined(separator: " | "))")
        }
    }
}

/// Sparkle's delegates. Sparkle calls them on the main thread.
@MainActor
private final class SparkleDelegate: NSObject, SPUUpdaterDelegate, @preconcurrency SPUStandardUserDriverDelegate {
    weak var owner: Updater?

    // Take over install-on-quit: no "ready to relaunch" alert, just a menu item.
    func updater(
        _ updater: SPUUpdater, willInstallUpdateOnQuit item: SUAppcastItem,
        immediateInstallationBlock immediateInstallHandler: @escaping () -> Void
    ) -> Bool {
        owner?.updateReadyOnQuit(version: item.displayVersionString, install: immediateInstallHandler)
        return true
    }

    func updater(_ updater: SPUUpdater, didAbortWithError error: Error) {
        Updater.testLog("aborted \(error.localizedDescription)")
    }

    func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck, error: Error?) {
        Updater.testLog("cycle finished \(error.map { $0.localizedDescription } ?? "ok")")
    }

    // Scheduled checks never pop a window; the update shows up in the app menu instead.
    var supportsGentleScheduledUpdateReminders: Bool { true }

    func standardUserDriverShouldHandleShowingScheduledUpdate(
        _ update: SUAppcastItem, andInImmediateFocus immediateFocus: Bool
    ) -> Bool {
        false
    }

    func standardUserDriverWillHandleShowingUpdate(
        _ handleShowingUpdate: Bool, forUpdate update: SUAppcastItem, state: SPUUserUpdateState
    ) {
        guard !handleShowingUpdate else { return }
        owner?.updateNeedsAttention(version: update.displayVersionString)
    }

    func standardUserDriverDidReceiveUserAttention(forUpdate update: SUAppcastItem) {
        owner?.updateGotAttention()
    }

    func standardUserDriverWillFinishUpdateSession() {
        owner?.updateGotAttention()
    }
}

/// App menu: "Check for Updates…" after "About dbear", plus "Restart to Update" once staged.
struct UpdateCommands: Commands {
    let updater: Updater

    var body: some Commands {
        CommandGroup(after: .appInfo) {
            Button("Check for Updates…") { updater.checkForUpdates() }
                .disabled(!updater.isEnabled || !updater.canCheckForUpdates)
            switch updater.pending {
            case .readyToInstall(let version):
                Button("Restart to Update to \(version)") { updater.installPendingUpdate() }
            case .available(let version):
                Button("Update to \(version)…") { updater.installPendingUpdate() }
            case nil:
                EmptyView()
            }
        }
    }
}
