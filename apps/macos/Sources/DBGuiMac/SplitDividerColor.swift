import AppKit
import SwiftUI

extension View {
    /// Draws `NavigationSplitView`'s column dividers in the system separator color, like `Divider()`
    /// and the toolbar line. AppKit's default for these dividers is near-black.
    func separatorColoredSplitDividers() -> some View {
        background(SplitDividerProbe().frame(width: 0, height: 0))
    }
}

private struct SplitDividerProbe: NSViewRepresentable {
    func makeNSView(context: Context) -> NSView { ProbeView() }
    func updateNSView(_ nsView: NSView, context: Context) {}

    /// SwiftUI owns the split view and AppKit has no public way to recolor its dividers.
    /// On macOS 26 each divider is an `NSSplitDividerView` subview with a `backgroundColor`,
    /// so we set that, and set it again whenever the split view re-lays out its panes
    /// (dividers can be recreated, e.g. when toggling the sidebar).
    private final class ProbeView: NSView {
        private var observed: [NSObjectProtocol] = []

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            observed.forEach(NotificationCenter.default.removeObserver)
            observed = []
            // The split view is created around us; wait for the hierarchy to settle.
            DispatchQueue.main.async { [weak self] in self?.attach() }
        }

        private func attach() {
            guard let root = window?.contentView else { return }
            for split in root.descendants(of: NSSplitView.self) {
                Self.recolorDividers(of: split)
                observed.append(NotificationCenter.default.addObserver(
                    forName: NSSplitView.didResizeSubviewsNotification, object: split, queue: .main
                ) { note in
                    guard let split = note.object as? NSSplitView else { return }
                    MainActor.assumeIsolated { Self.recolorDividers(of: split) }
                })
            }
        }

        private static let dividerClass: AnyClass? = NSClassFromString("NSSplitDividerView")
        private static let setBackgroundColor = NSSelectorFromString("setBackgroundColor:")

        private static func recolorDividers(of split: NSSplitView) {
            guard let dividerClass else { return }
            for divider in split.subviews where divider.isKind(of: dividerClass) && divider.responds(to: setBackgroundColor) {
                divider.perform(setBackgroundColor, with: NSColor.separatorColor)
                divider.needsDisplay = true
            }
        }
    }
}

private extension NSView {
    func descendants<T: NSView>(of type: T.Type) -> [T] {
        subviews.flatMap { child -> [T] in
            ((child as? T).map { [$0] } ?? []) + child.descendants(of: type)
        }
    }
}
