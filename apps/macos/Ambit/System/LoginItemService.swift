// Launch at login through `SMAppService.mainApp`.
//
// - First run registers the login item once and records `launchAtLoginInitialized`, so later
//   launches never re-register behind the user's back.
// - The Settings toggle registers or unregisters and records the choice in
//   `launchAtLoginPreferred` (default true).
// - The status shown is what macOS reports: enabled, waiting for approval in System Settings, or
//   turned off there although the user asked for it.
//
// A login launch is detected in `AppDelegate` from the launch Apple event (see `LoginLaunch`).

import Foundation
import Observation
import OSLog
import ServiceManagement

enum LoginItemStatus: Equatable, Sendable {
    case enabled
    case notRegistered
    /// Registered, but the user must allow it in System Settings > General > Login Items.
    case requiresApproval
    /// macOS cannot find the app's login item (for example, a copy run from a disk image).
    case notFound
}

/// The system login item, injectable for tests.
@MainActor
protocol LoginItemControl: AnyObject {
    var status: LoginItemStatus { get }
    func register() throws
    func unregister() throws
    func openSystemSettings()
}

/// `SMAppService.mainApp`.
@MainActor
final class SystemLoginItemControl: LoginItemControl {
    private var service: SMAppService { .mainApp }

    var status: LoginItemStatus {
        switch service.status {
        case .enabled: .enabled
        case .requiresApproval: .requiresApproval
        case .notFound: .notFound
        case .notRegistered: .notRegistered
        @unknown default: .notRegistered
        }
    }

    func register() throws {
        try service.register()
    }

    func unregister() throws {
        try service.unregister()
    }

    func openSystemSettings() {
        SMAppService.openSystemSettingsLoginItems()
    }
}

/// For unit tests and UI tests, which must never touch the user's real login items.
@MainActor
final class InMemoryLoginItemControl: LoginItemControl {
    var status: LoginItemStatus
    var registerError: (any Error)?
    /// What `register()` leaves behind, to simulate macOS asking for approval.
    var statusAfterRegister: LoginItemStatus = .enabled
    private(set) var registerCount = 0
    private(set) var unregisterCount = 0
    private(set) var openedSystemSettings = 0

    init(status: LoginItemStatus = .notRegistered) {
        self.status = status
    }

    func register() throws {
        registerCount += 1
        if let registerError {
            throw registerError
        }
        status = statusAfterRegister
    }

    func unregister() throws {
        unregisterCount += 1
        status = .notRegistered
    }

    func openSystemSettings() {
        openedSystemSettings += 1
    }
}

@MainActor
@Observable
final class LoginItemService {
    @ObservationIgnored private let control: any LoginItemControl
    @ObservationIgnored private let store: AppStateStore

    private(set) var status: LoginItemStatus
    /// The user's choice (true by default).
    private(set) var isPreferred: Bool
    /// The last register/unregister failure, shown under the toggle.
    private(set) var lastError: String?

    init(control: any LoginItemControl, store: AppStateStore) {
        self.control = control
        self.store = store
        status = control.status
        isPreferred = store.state.launchAtLoginPreferred
    }

    /// The toggle's value: on when registered, including while macOS awaits approval.
    var isEnabled: Bool {
        status == .enabled || status == .requiresApproval
    }

    /// A note for Settings when macOS does not run Ambit at login as the user asked.
    var notice: String? {
        switch status {
        case .requiresApproval:
            String(localized: "macOS needs your approval in System Settings › General › Login Items.")
        case .notRegistered where isPreferred:
            String(localized: "Launch at login is turned off in System Settings.")
        case .notFound:
            String(localized: "macOS cannot register this copy of Ambit. Move Ambit to the Applications folder.")
        default:
            nil
        }
    }

    var showsSystemSettingsAction: Bool {
        notice != nil && status != .notFound
    }

    /// Registers the login item on the very first run only.
    func applyFirstRunDefault() {
        defer { refresh() }
        guard !store.state.launchAtLoginInitialized else {
            return
        }

        if store.state.launchAtLoginPreferred {
            do {
                try control.register()
            } catch {
                Logger.loginItem.error("Could not register the login item: \(error, privacy: .public)")
                lastError = error.localizedDescription
            }
        }
        persist { $0.launchAtLoginInitialized = true }
    }

    func setEnabled(_ enabled: Bool) {
        lastError = nil
        do {
            if enabled {
                try control.register()
            } else {
                try control.unregister()
            }
        } catch {
            Logger.loginItem.error("Could not change the login item: \(error, privacy: .public)")
            lastError = error.localizedDescription
        }
        persist {
            $0.launchAtLoginPreferred = enabled
            $0.launchAtLoginInitialized = true
        }
        refresh()
    }

    /// Rereads the status; the user can change it in System Settings at any time.
    func refresh() {
        status = control.status
        isPreferred = store.state.launchAtLoginPreferred
    }

    func openSystemSettings() {
        control.openSystemSettings()
    }

    private func persist(_ change: (inout AppState) -> Void) {
        do {
            try store.update(change)
        } catch {
            lastError = error.localizedDescription
        }
    }
}

extension Logger {
    static let loginItem = Logger(subsystem: "com.nebulab.ambit", category: "login-item")
}
