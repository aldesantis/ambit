// The live updater: Sparkle 2 with a custom user driver, so Sparkle never shows its own windows
// and the app presents update status in the menu bar and Settings instead.
//
// Two Sparkle paths lead to a staged update:
// - Scheduled/background checks (launch, then every `AppUpdates.checkInterval`) download silently
//   through Sparkle's automatic driver. The delegate hears `willDownloadUpdate` (indeterminate
//   progress) and then `willInstallUpdateOnQuit`, where we keep the immediate-install block for
//   "Restart to update" and return true. Sparkle installs on quit either way.
// - User-initiated checks (and retries) go through this user driver: the found update is
//   downloaded right away (reply `.install`), with byte progress, and `showReadyToInstallAndRelaunch`
//   hands us a reply we hold until "Restart to update".
// A found update that is already installing in the background is never relaunched here: replying
// `.install` at that stage relaunches immediately, so that reply is held like the others.
//
// Failures (`showUpdaterError`, `didAbortWithError`, `failedToDownloadUpdate`) leave the current
// version running and show `.failed`, from which `checkForUpdates()` retries.

import AppKit
import Observation
import OSLog
import Sparkle

@MainActor
@Observable
final class SparkleAppUpdater: NSObject, AppUpdater {
    private(set) var state: AppUpdateState
    private(set) var lastCheckedAt: Date?

    @ObservationIgnored private let bundle: Bundle
    @ObservationIgnored private var updater: SPUUpdater?
    /// Installs the staged update and relaunches; set only in `.readyToRestart`.
    @ObservationIgnored private var installNow: (() -> Void)?
    /// The version being downloaded or staged, for status text.
    @ObservationIgnored private var pendingVersion: String?
    @ObservationIgnored private var expectedLength: UInt64 = 0
    @ObservationIgnored private var receivedLength: UInt64 = 0

    init(bundle: Bundle = .main) {
        self.bundle = bundle
        state = Self.isConfigured(bundle)
            ? .idle
            : .unavailable(reason: String(localized: "App updates are not configured in this build"))
        super.init()
    }

    /// Release builds carry a feed URL and an EdDSA public key; local builds leave the key empty.
    static func isConfigured(_ bundle: Bundle) -> Bool {
        func value(_ key: String) -> String {
            (bundle.object(forInfoDictionaryKey: key) as? String ?? "").trimmingCharacters(in: .whitespaces)
        }
        return !value("SUFeedURL").isEmpty && !value("SUPublicEDKey").isEmpty
    }

    func start() {
        guard updater == nil else {
            return
        }
        if case .unavailable = state {
            return
        }

        let updater = SPUUpdater(hostBundle: bundle, applicationBundle: bundle, userDriver: self, delegate: self)
        updater.automaticallyChecksForUpdates = true
        updater.automaticallyDownloadsUpdates = true
        updater.updateCheckInterval = AppUpdates.checkInterval
        do {
            try updater.start()
        } catch {
            Logger.updates.error("Sparkle did not start: \(error, privacy: .public)")
            state = .unavailable(reason: String(localized: "App updates are unavailable: \(error.localizedDescription)"))
            return
        }
        self.updater = updater
        lastCheckedAt = updater.lastUpdateCheckDate
        // The scheduled checks start one interval after the previous one; check at launch too.
        updater.checkForUpdatesInBackground()
    }

    func checkForUpdates() {
        guard state.canCheck, let updater, updater.canCheckForUpdates else {
            return
        }
        state = .checking
        updater.checkForUpdates()
    }

    func installAndRelaunch() {
        guard state.canRestartToUpdate, let installNow else {
            return
        }
        self.installNow = nil
        state = .installing
        installNow()
    }

    // MARK: State changes shared by both paths

    private func staged(version: String?, install: @escaping () -> Void) {
        installNow = install
        state = .readyToRestart(version: version ?? pendingVersion)
    }

    private func failed(_ error: any Error) {
        let nsError = error as NSError
        if nsError.domain == SUSparkleErrorDomain, nsError.code == Int(SUError.noUpdateError.rawValue) {
            finishedCheck(found: false)
            return
        }
        // A staged update stays staged: a later background error must not hide it.
        if state.canRestartToUpdate {
            return
        }
        Logger.updates.error("App update failed: \(error, privacy: .public)")
        installNow = nil
        state = .failed(message: error.localizedDescription)
    }

    private func finishedCheck(found: Bool) {
        lastCheckedAt = updater?.lastUpdateCheckDate ?? .now
        if !found, !state.canRestartToUpdate {
            state = .upToDate
        }
    }

    private func version(of item: SUAppcastItem) -> String? {
        let display = item.displayVersionString
        return display.isEmpty ? (item.versionString.isEmpty ? nil : item.versionString) : display
    }
}

// MARK: - SPUUserDriver (user-initiated sessions)

extension SparkleAppUpdater: SPUUserDriver {
    func show(
        _ request: SPUUpdatePermissionRequest, reply: @escaping (SUUpdatePermissionResponse) -> Void
    ) {
        // Info.plist enables automatic checks, so this is only a fallback; the issue requires them.
        reply(SUUpdatePermissionResponse(automaticUpdateChecks: true, sendSystemProfile: false))
    }

    func showUserInitiatedUpdateCheck(cancellation: @escaping () -> Void) {
        state = .checking
    }

    func showUpdateFound(
        with appcastItem: SUAppcastItem, state userState: SPUUserUpdateState,
        reply: @escaping (SPUUserUpdateChoice) -> Void
    ) {
        lastCheckedAt = updater?.lastUpdateCheckDate ?? .now
        pendingVersion = version(of: appcastItem)

        if appcastItem.isInformationOnlyUpdate {
            // Nothing to download; there is no in-app action for it.
            reply(.dismiss)
            finishedCheck(found: false)
            return
        }

        switch userState.stage {
        case .notDownloaded, .downloaded:
            // Download (or unpack) right away; installing still waits for the user or a quit.
            state = .downloading(version: pendingVersion, fraction: nil)
            reply(.install)
        case .installing:
            // `.install` now would relaunch at once; hold it for "Restart to update".
            staged(version: pendingVersion) { reply(.install) }
        @unknown default:
            reply(.dismiss)
        }
    }

    func showUpdateReleaseNotes(with downloadData: SPUDownloadData) {}

    func showUpdateReleaseNotesFailedToDownloadWithError(_ error: any Error) {}

    func showUpdateNotFoundWithError(_ error: any Error, acknowledgement: @escaping () -> Void) {
        finishedCheck(found: false)
        acknowledgement()
    }

    func showUpdaterError(_ error: any Error, acknowledgement: @escaping () -> Void) {
        failed(error)
        acknowledgement()
    }

    func showDownloadInitiated(cancellation: @escaping () -> Void) {
        expectedLength = 0
        receivedLength = 0
        state = .downloading(version: pendingVersion, fraction: 0)
    }

    func showDownloadDidReceiveExpectedContentLength(_ expectedContentLength: UInt64) {
        expectedLength = expectedContentLength
        receivedLength = 0
        state = .downloading(version: pendingVersion, fraction: expectedLength > 0 ? 0 : nil)
    }

    func showDownloadDidReceiveData(ofLength length: UInt64) {
        receivedLength += length
        guard expectedLength > 0 else {
            state = .downloading(version: pendingVersion, fraction: nil)
            return
        }
        state = .downloading(
            version: pendingVersion, fraction: min(1, Double(receivedLength) / Double(expectedLength)))
    }

    func showDownloadDidStartExtractingUpdate() {
        state = .preparing(version: pendingVersion, fraction: 0)
    }

    func showExtractionReceivedProgress(_ progress: Double) {
        state = .preparing(version: pendingVersion, fraction: min(1, max(0, progress)))
    }

    func showReady(toInstallAndRelaunch reply: @escaping (SPUUserUpdateChoice) -> Void) {
        // Held until "Restart to update". If the app quits first, Sparkle installs on quit.
        staged(version: pendingVersion) { reply(.install) }
    }

    func showInstallingUpdate(withApplicationTerminated applicationTerminated: Bool, retryTerminatingApplication: @escaping () -> Void) {
        // Termination goes through AppDelegate's guards; never retry it behind the user's back.
        state = .installing
    }

    func showUpdateInstalledAndRelaunched(_ relaunched: Bool, acknowledgement: @escaping () -> Void) {
        acknowledgement()
    }

    func showUpdateInFocus() {}

    func dismissUpdateInstallation() {
        // The session ended. Transient states fall back; results and staged updates stay.
        switch state {
        case .checking:
            state = lastCheckedAt == nil ? .idle : .upToDate
        case .downloading, .preparing:
            if installNow == nil {
                state = .idle
            }
        default:
            break
        }
    }
}

// MARK: - SPUUpdaterDelegate (background sessions and outcomes)

extension SparkleAppUpdater: SPUUpdaterDelegate {
    func updater(_ updater: SPUUpdater, willDownloadUpdate item: SUAppcastItem, with request: NSMutableURLRequest) {
        pendingVersion = version(of: item)
        if !state.canRestartToUpdate, state.progress == nil {
            state = .downloading(version: pendingVersion, fraction: nil)
        }
    }

    func updater(_ updater: SPUUpdater, failedToDownloadUpdate item: SUAppcastItem, error: any Error) {
        failed(error)
    }

    func updater(
        _ updater: SPUUpdater, willInstallUpdateOnQuit item: SUAppcastItem,
        immediateInstallationBlock immediateInstallHandler: @escaping () -> Void
    ) -> Bool {
        // Take over the relaunch: the block runs only from "Restart to update". Sparkle still
        // installs on a normal quit.
        staged(version: version(of: item), install: immediateInstallHandler)
        return true
    }

    func updaterDidNotFindUpdate(_ updater: SPUUpdater, error: any Error) {
        finishedCheck(found: false)
    }

    func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        lastCheckedAt = updater.lastUpdateCheckDate ?? .now
        pendingVersion = version(of: item)
    }

    func updater(_ updater: SPUUpdater, didAbortWithError error: any Error) {
        failed(error)
    }

    func updaterShouldRelaunchApplication(_ updater: SPUUpdater) -> Bool {
        true
    }
}

extension Logger {
    static let updates = Logger(subsystem: "com.nebulab.ambit", category: "updates")
}
