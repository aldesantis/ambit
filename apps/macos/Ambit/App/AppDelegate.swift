import AppKit

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationWillFinishLaunching(_ notification: Notification) {
        guard !LaunchContext.current.isUnitTestHost else {
            return
        }

        if let other = Self.otherInstance() {
            other.activate()
            NSApp.terminate(nil)
        }
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
