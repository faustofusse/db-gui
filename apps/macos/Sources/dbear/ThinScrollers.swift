import AppKit
import ObjectiveC

/// Slim, dim overlay scroller: a thin rounded knob and no track, whatever the system's
/// "Show scroll bars" setting. Used by every scroll view in the app (see `ThinScrollers.install`).
final class ThinScroller: NSScroller {
    private static let thickness: CGFloat = 5
    private static let inset: CGFloat = 2

    override class var isCompatibleWithOverlayScrollers: Bool { true }

    override class func scrollerWidth(for controlSize: NSControl.ControlSize, scrollerStyle: NSScroller.Style) -> CGFloat {
        thickness + inset * 2
    }

    override func drawKnobSlot(in slotRect: NSRect, highlight flag: Bool) {}

    override func drawKnob() {
        let knob = rect(for: .knob)
        guard knob.width > 0, knob.height > 0 else { return }
        let (t, i) = (Self.thickness, Self.inset)
        let r = bounds.height > bounds.width
            ? NSRect(x: knob.maxX - t - i, y: knob.minY + i, width: t, height: knob.height - i * 2)
            : NSRect(x: knob.minX + i, y: knob.maxY - t - i, width: knob.width - i * 2, height: t)
        // Dim: present enough to show position, never competing with content.
        NSColor.labelColor.withAlphaComponent(0.16).setFill()
        NSBezierPath(roundedRect: r, xRadius: t / 2, yRadius: t / 2).fill()
    }
}

enum ThinScrollers {
    /// Makes every `NSScrollView` (AppKit's and the ones behind SwiftUI's List, ScrollView and
    /// TextEditor) use `ThinScroller` overlay scrollers. Call once at launch.
    @MainActor
    static func install() {
        // Overlay style app-wide. The argument domain isn't persisted and wins over the global
        // "Show scroll bars" preference; it must be set before AppKit first reads it.
        var arguments = UserDefaults.standard.volatileDomain(forName: UserDefaults.argumentDomain)
        arguments["AppleShowScrollBars"] = "WhenScrolling"
        UserDefaults.standard.setVolatileDomain(arguments, forName: UserDefaults.argumentDomain)

        // Swap in our scrollers whenever a scroll view joins a window. Method swizzling on
        // NSScrollView itself (no isa changes), so SwiftUI's own subclasses keep working.
        let cls: AnyClass = NSScrollView.self
        let originalSelector = #selector(NSView.viewDidMoveToWindow)
        let hookSelector = #selector(NSScrollView.dbgui_viewDidMoveToWindow)
        guard let original = class_getInstanceMethod(cls, originalSelector),
              let hook = class_getInstanceMethod(cls, hookSelector)
        else { return }
        // NSScrollView inherits viewDidMoveToWindow from NSView. Exchanging directly would
        // swap NSView's implementation for every view, so give NSScrollView its own override
        // first, then point the hook selector at the inherited implementation.
        if class_addMethod(cls, originalSelector, method_getImplementation(hook), method_getTypeEncoding(hook)) {
            class_replaceMethod(cls, hookSelector, method_getImplementation(original), method_getTypeEncoding(original))
        } else {
            method_exchangeImplementations(original, hook)
        }
    }
}

extension NSScrollView {
    @objc fileprivate func dbgui_viewDidMoveToWindow() {
        dbgui_viewDidMoveToWindow() // the original implementation (swapped)
        guard window != nil else { return }
        useThinScrollers()
    }

    func useThinScrollers() {
        var changed = false
        if !(verticalScroller is ThinScroller) {
            verticalScroller = ThinScroller()
            changed = true
        }
        if !(horizontalScroller is ThinScroller) {
            horizontalScroller = ThinScroller()
            changed = true
        }
        if scrollerStyle != .overlay {
            scrollerStyle = .overlay
            changed = true
        }
        if changed { tile() }
    }
}
