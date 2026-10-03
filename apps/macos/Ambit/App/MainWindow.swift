// AppKit glue for the SwiftUI main window: find its NSWindow and put a close guard in front of
// SwiftUI's window delegate, since SwiftUI offers no way to veto closing a window.

import AppKit
import SwiftUI

/// Calls `onWindow` with the hosting NSWindow once the view is in one.
struct WindowAccessor: NSViewRepresentable {
    var onWindow: @MainActor (NSWindow) -> Void

    func makeNSView(context: Context) -> AccessorView {
        let view = AccessorView()
        view.onWindow = onWindow
        return view
    }

    func updateNSView(_ view: AccessorView, context: Context) {
        view.onWindow = onWindow
    }

    final class AccessorView: NSView {
        var onWindow: (@MainActor (NSWindow) -> Void)?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let window {
                onWindow?(window)
            }
        }
    }
}

/// Sits in front of SwiftUI's window delegate: answers `windowShouldClose` and forwards every
/// other delegate message to the original delegate.
final class WindowCloseGuard: NSObject, NSWindowDelegate {
    nonisolated(unsafe) private weak var original: (any NSWindowDelegate)?
    private let shouldClose: @MainActor (NSWindow) -> Bool

    /// Installs a guard on `window` unless one is already there. Keep the returned object alive.
    @MainActor
    static func install(on window: NSWindow, shouldClose: @escaping @MainActor (NSWindow) -> Bool) -> WindowCloseGuard? {
        if window.delegate is WindowCloseGuard {
            return nil
        }
        let guardian = WindowCloseGuard(original: window.delegate, shouldClose: shouldClose)
        window.delegate = guardian
        return guardian
    }

    private init(original: (any NSWindowDelegate)?, shouldClose: @escaping @MainActor (NSWindow) -> Bool) {
        self.original = original
        self.shouldClose = shouldClose
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard shouldClose(sender) else {
            return false
        }
        return original?.windowShouldClose?(sender) ?? true
    }

    override func responds(to selector: Selector!) -> Bool {
        super.responds(to: selector) || (original?.responds(to: selector) ?? false)
    }

    override func forwardingTarget(for selector: Selector!) -> Any? {
        if let original, original.responds(to: selector) {
            return original
        }
        return super.forwardingTarget(for: selector)
    }
}

extension NSWindow {
    /// A window that counts as "open" for the Dock icon: the main window or Settings, not the
    /// menu bar extra, panels or status items.
    var isAppWindow: Bool {
        (isVisible || isMiniaturized) && styleMask.contains(.titled) && !(self is NSPanel) && level == .normal
    }
}
