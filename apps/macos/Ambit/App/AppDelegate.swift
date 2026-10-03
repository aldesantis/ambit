import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    /// Set by `AmbitApp.init`, which runs before AppKit sends any delegate message.
    static var lifecycle: AppLifecycle?

    private var closeGuard: WindowCloseGuard?
    private var observers: [any NSObjectProtocol] = []

    private var lifecycle: AppLifecycle? { Self.lifecycle }

    func applicationWillFinishLaunching(_ notification: Notification) {
        guard !LaunchContext.current.isUnitTestHost else {
            return
        }

        if let other = Self.otherInstance() {
            // Reopening the running copy makes it show its window, like any normal launch.
            if let url = other.bundleURL {
                NSWorkspace.shared.openApplication(at: url, configuration: NSWorkspace.OpenConfiguration())
            } else {
                other.activate()
            }
            NSApp.terminate(nil)
        }
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        let launch = LaunchContext.current
        guard !launch.isUnitTestHost else {
            return
        }
        let isLoginLaunch = launch.isLoginLaunch || (!launch.isUITesting && LoginLaunch.isCurrentAppleEventLoginLaunch())
        lifecycle?.didFinishLaunching(isLoginLaunch: isLoginLaunch)

        if isLoginLaunch {
            // The scene suppresses the window for `--login-launch`; with the Apple event SwiftUI
            // may already have opened it.
            for window in NSApp.windows where window.isAppWindow {
                window.close()
            }
        }

        let center = NotificationCenter.default
        let names: [Notification.Name] = [
            NSWindow.didBecomeKeyNotification, NSWindow.willCloseNotification,
            NSWindow.didMiniaturizeNotification, NSWindow.didDeminiaturizeNotification,
            NSWindow.didChangeOcclusionStateNotification,
        ]
        for name in names {
            observers.append(
                center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                    // After `willClose` the window is still visible; look once it is gone.
                    DispatchQueue.main.async {
                        MainActor.assumeIsolated { self?.updateActivationPolicy() }
                    }
                })
        }
        // A normal launch stays `.regular` while SwiftUI opens the window; checking now, before
        // the window exists, would hide the Dock icon and keep the window from becoming key.
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        // `flag` also counts the menu bar extra's windows, so look at app windows only.
        if !NSApp.windows.contains(where: \.isAppWindow) {
            lifecycle?.openMainWindow()
        }
        return false
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard let lifecycle else {
            return .terminateNow
        }
        return lifecycle.terminationReply { approved in
            NSApp.reply(toApplicationShouldTerminate: approved)
        }
    }

    /// Called by the main window's content once it is in a window.
    func attach(mainWindow window: NSWindow) {
        guard let lifecycle else {
            return
        }
        if let installed = WindowCloseGuard.install(on: window, shouldClose: { [weak lifecycle] window in
            lifecycle?.shouldCloseMainWindow { window.close() } ?? true
        }) {
            closeGuard = installed
        }
        updateActivationPolicy()
    }

    private func updateActivationPolicy() {
        lifecycle?.windowsChanged(hasVisibleWindow: NSApp.windows.contains { $0.isAppWindow })
    }

    /// Another running Ambit, matched by bundle id so copies at different paths count too.
    private static func otherInstance() -> NSRunningApplication? {
        let current = NSRunningApplication.current
        guard let bundleID = current.bundleIdentifier else {
            return nil
        }

        return NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
            .first { $0.processIdentifier != current.processIdentifier && !$0.isTerminated }
    }
}
