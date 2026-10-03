// App-level lifecycle rules (issue #59, "Window, menu bar, and app updates"):
//
// - Quit and "Restart to update" first wait for an in-flight apply to finish, then ask to resolve
//   pending drafts (Apply / Discard / Cancel). Cancel keeps the app running and the draft open.
//   A draft is never discarded for an app update.
// - Closing the main window with a draft asks the same question; the app keeps running in the
//   menu bar afterwards.
// - The Dock icon shows while an app window is open and hides when only the menu bar remains.
// - A login launch starts in the menu bar without opening the window.
//
// Drafts and operations belong to the setup stores; this file sees them only through
// `PendingChangesGuard`, so it does not depend on how they are implemented.

import AppKit
import Observation

/// Why the app asks to resolve pending changes.
enum LeaveReason: Sendable, Equatable {
    case quit
    case restartToUpdate
    case closeWindow
}

/// Pending drafts and running operations, as the lifecycle needs them. `AppModel` conforms once
/// the setup stores provide these (see `AmbitApp.init`).
@MainActor
protocol PendingChangesGuard: AnyObject {
    /// Some setup has unapplied edits.
    var hasPendingChanges: Bool { get }
    /// An apply (or another operation that must not be interrupted) is writing right now.
    var isApplying: Bool { get }
    /// Offers Apply / Discard / Cancel for every pending draft. True when the caller may go on
    /// (applied successfully or discarded); false on Cancel or a failed apply.
    func resolvePendingChangesForLeaving(_ reason: LeaveReason) async -> Bool
    /// Returns once `isApplying` is false.
    func waitWhileApplying() async
}

extension PendingChangesGuard {
    func waitWhileApplying() async {
        while isApplying {
            try? await Task.sleep(for: .milliseconds(100))
        }
    }
}

/// Used until the setup stores conform: nothing is ever pending.
@MainActor
final class NoPendingChanges: PendingChangesGuard {
    var hasPendingChanges: Bool { false }
    var isApplying: Bool { false }
    func resolvePendingChangesForLeaving(_ reason: LeaveReason) async -> Bool { true }
}

@MainActor
@Observable
final class AppLifecycle {
    let updater: any AppUpdater
    let loginItem: LoginItemService
    @ObservationIgnored var pendingChanges: any PendingChangesGuard

    /// Set while a quit, restart or close waits on an apply or a draft question.
    private(set) var leaving: LeaveReason?
    /// True while waiting for an in-flight apply before leaving.
    private(set) var isWaitingForOperation = false

    /// Set once leaving is approved, so the termination that follows is not asked again.
    @ObservationIgnored private(set) var terminationApproved = false
    /// Opens (or focuses) the main window. Registered by a SwiftUI view with `openWindow`.
    @ObservationIgnored var openMainWindowAction: (() -> Void)?
    /// Replaced in tests.
    @ObservationIgnored var setActivationPolicy: (NSApplication.ActivationPolicy) -> Void = { policy in
        guard NSApp.activationPolicy() != policy else {
            return
        }
        NSApp.setActivationPolicy(policy)
    }

    init(updater: any AppUpdater, loginItem: LoginItemService, pendingChanges: any PendingChangesGuard = NoPendingChanges()) {
        self.updater = updater
        self.loginItem = loginItem
        self.pendingChanges = pendingChanges
    }

    convenience init(environment: AppEnvironment) {
        self.init(
            updater: environment.updater,
            loginItem: LoginItemService(control: environment.loginItems, store: environment.stateStore))
    }

    /// Called once at launch, after AppKit finished launching.
    func didFinishLaunching(isLoginLaunch: Bool) {
        loginItem.applyFirstRunDefault()
        updater.start()
        if isLoginLaunch {
            setActivationPolicy(.accessory)
        }
    }

    // MARK: Window and Dock

    func openMainWindow() {
        setActivationPolicy(.regular)
        openMainWindowAction?()
        NSApp.activate()
    }

    /// The Dock icon follows the app's windows (main window or Settings).
    func windowsChanged(hasVisibleWindow: Bool) {
        setActivationPolicy(hasVisibleWindow ? .regular : .accessory)
    }

    /// Whether the main window may close now. With a draft, returns false and calls `close`
    /// later if the user applies or discards.
    func shouldCloseMainWindow(close: @escaping @MainActor () -> Void) -> Bool {
        guard pendingChanges.hasPendingChanges else {
            return true
        }
        guard leaving == nil else {
            return false
        }

        Task {
            leaving = .closeWindow
            defer { leaving = nil }
            if await pendingChanges.resolvePendingChangesForLeaving(.closeWindow) {
                close()
            }
        }
        return false
    }

    // MARK: Quit and restart

    /// `applicationShouldTerminate`: terminates now when nothing is pending; otherwise asks and
    /// answers through `reply` later.
    func terminationReply(reply: @escaping @MainActor (Bool) -> Void) -> NSApplication.TerminateReply {
        if terminationApproved || (!pendingChanges.hasPendingChanges && !pendingChanges.isApplying) {
            return .terminateNow
        }
        guard leaving == nil else {
            return .terminateCancel
        }

        Task {
            let approved = await prepareToLeave(for: .quit)
            reply(approved)
        }
        return .terminateLater
    }

    /// "Restart to update": runs only from a staged update and after the guards pass.
    func restartToUpdate() async {
        guard updater.state.canRestartToUpdate, leaving == nil else {
            return
        }
        guard await prepareToLeave(for: .restartToUpdate) else {
            return
        }
        // Sparkle terminates the app to install; that termination is already approved.
        updater.installAndRelaunch()
    }

    /// Waits for an in-flight apply, then resolves drafts. True when the app may quit.
    func prepareToLeave(for reason: LeaveReason) async -> Bool {
        guard leaving == nil else {
            return false
        }
        leaving = reason
        defer { leaving = nil }

        await waitForOperation()
        if pendingChanges.hasPendingChanges {
            openMainWindow()
            guard await pendingChanges.resolvePendingChangesForLeaving(reason) else {
                return false
            }
            // Apply from the question runs an operation; let it finish.
            await waitForOperation()
        }
        terminationApproved = true
        return true
    }

    private func waitForOperation() async {
        guard pendingChanges.isApplying else {
            return
        }
        isWaitingForOperation = true
        defer { isWaitingForOperation = false }
        await pendingChanges.waitWhileApplying()
    }
}

/// Detects a launch by macOS at login.
///
/// Choice: the launch Apple event. macOS opens login items with a `kAEOpenApplication` event
/// whose `keyAEPropData` is `keyAELaunchedAsLogInItem`; it is current while
/// `applicationDidFinishLaunching` runs. `SMAppService.mainApp` gives no launch argument, so the
/// event is the only signal for the main app. UI tests pass `--login-launch` instead
/// (`LaunchContext.isLoginLaunch`).
enum LoginLaunch {
    @MainActor
    static func isCurrentAppleEventLoginLaunch() -> Bool {
        guard let event = NSAppleEventManager.shared().currentAppleEvent,
            event.eventID == AEEventID(kAEOpenApplication)
        else {
            return false
        }
        return event.paramDescriptor(forKeyword: AEKeyword(keyAEPropData))?.enumCodeValue
            == OSType(keyAELaunchedAsLogInItem)
    }
}
