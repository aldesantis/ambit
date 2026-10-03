// App updates, as the rest of the app sees them. `SparkleAppUpdater` is the live implementation
// and `FakeAppUpdater` drives tests and UI tests. Views and the lifecycle only use this protocol.
//
// Rules (issue #59): check at launch and every six hours, download automatically, show progress,
// offer "Restart to update", install a downloaded update on a normal quit. The updater itself never
// relaunches the app: only `installAndRelaunch()` does, and `AppLifecycle` calls it after the
// pending-changes and in-flight-operation guards pass. Updating the app never touches catalogs.

import Foundation
import Observation

enum AppUpdateState: Equatable, Sendable {
    /// Updates are not configured in this build (no feed key), or the updater failed to start.
    case unavailable(reason: String)
    /// No check has finished since launch.
    case idle
    case checking
    case upToDate
    /// `fraction` is nil while the size is unknown (silent background downloads report none).
    case downloading(version: String?, fraction: Double?)
    /// Unpacking and verifying the downloaded update.
    case preparing(version: String?, fraction: Double?)
    /// Downloaded and staged: installs on quit, or now through "Restart to update".
    case readyToRestart(version: String?)
    /// The app is about to quit so the update can install.
    case installing
    /// The current version keeps working; `retry` checks again.
    case failed(message: String)
}

extension AppUpdateState {
    /// A user-initiated check (or retry) makes sense.
    var canCheck: Bool {
        switch self {
        case .idle, .upToDate, .failed: true
        default: false
        }
    }

    var canRestartToUpdate: Bool {
        if case .readyToRestart = self { true } else { false }
    }

    var isFailed: Bool {
        if case .failed = self { true } else { false }
    }

    /// Download or preparation progress; nil when there is none to show or it is indeterminate.
    var progress: Double? {
        switch self {
        case let .downloading(_, fraction), let .preparing(_, fraction): fraction
        default: nil
        }
    }

    var isBusy: Bool {
        switch self {
        case .checking, .downloading, .preparing, .installing: true
        default: false
        }
    }

    /// One line for the menu bar and Settings.
    var statusText: String {
        switch self {
        case let .unavailable(reason):
            reason
        case .idle:
            String(localized: "App updates not checked yet")
        case .checking:
            String(localized: "Checking for app updates…")
        case .upToDate:
            String(localized: "Ambit is up to date")
        case let .downloading(version, fraction):
            Self.withPercent(
                version.map { String(localized: "Downloading Ambit \($0)…") }
                    ?? String(localized: "Downloading update…"),
                fraction)
        case let .preparing(version, fraction):
            Self.withPercent(
                version.map { String(localized: "Preparing Ambit \($0)…") }
                    ?? String(localized: "Preparing update…"),
                fraction)
        case let .readyToRestart(version):
            version.map { String(localized: "Ambit \($0) is ready to install") }
                ?? String(localized: "An update is ready to install")
        case .installing:
            String(localized: "Installing update…")
        case let .failed(message):
            String(localized: "App update failed: \(message)")
        }
    }

    private static func withPercent(_ text: String, _ fraction: Double?) -> String {
        guard let fraction else {
            return text
        }
        return "\(text) \(Int((fraction * 100).rounded()))%"
    }
}

@MainActor
protocol AppUpdater: AnyObject, Observable {
    var state: AppUpdateState { get }
    /// When the last check finished, if known.
    var lastCheckedAt: Date? { get }

    /// Starts the updater: an immediate background check, then one every six hours.
    func start()
    /// A user-initiated check; also the retry after a failure. Downloads automatically.
    func checkForUpdates()
    /// Quits, installs the staged update and relaunches. Only `AppLifecycle` calls this, after
    /// its guards pass; it does nothing unless `state.canRestartToUpdate`.
    func installAndRelaunch()
}

enum AppUpdates {
    /// Six hours, the issue's check interval.
    static let checkInterval: TimeInterval = 6 * 60 * 60
}
