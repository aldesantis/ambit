import Foundation
import Sparkle
import Testing

@testable import Ambit

@MainActor
struct FakeAppUpdaterTests {
    @Test func noUpdateIsUpToDate() {
        let updater = FakeAppUpdater(scenario: .none)

        updater.start()

        #expect(updater.state == .upToDate)
        #expect(updater.state.canCheck)
        #expect(!updater.state.canRestartToUpdate)
        #expect(updater.lastCheckedAt != nil)
    }

    @Test func anAvailableUpdateDownloadsAndWaitsForRestart() {
        let updater = FakeAppUpdater(scenario: .available)

        updater.start()

        #expect(updater.state == .readyToRestart(version: FakeAppUpdater.version))
        #expect(!updater.state.canCheck)
        #expect(updater.installCount == 0)
    }

    @Test func aFailedDownloadKeepsTheCurrentVersionAndRetries() {
        let updater = FakeAppUpdater(scenario: .downloadFailed)

        updater.start()

        #expect(updater.state.isFailed)
        #expect(updater.state.canCheck)
        #expect(updater.installCount == 0)

        updater.checkForUpdates()

        #expect(updater.state == .readyToRestart(version: FakeAppUpdater.version))
        #expect(updater.checkCount == 2)
    }

    @Test func aStagedUpdateInstallsOnlyWhenAsked() {
        let updater = FakeAppUpdater(scenario: .staged)

        updater.start()
        #expect(updater.state.canRestartToUpdate)
        #expect(updater.checkCount == 0)

        updater.installAndRelaunch()

        #expect(updater.installCount == 1)
        #expect(updater.state == .installing)
    }

    @Test func installingWithoutAStagedUpdateDoesNothing() {
        let updater = FakeAppUpdater(scenario: .none)
        updater.start()

        updater.installAndRelaunch()

        #expect(updater.installCount == 0)
        #expect(updater.state == .upToDate)
    }

    @Test func unknownScenarioNamesFallBackToNone() {
        #expect(FakeAppUpdater(named: "bogus").scenario == .none)
        #expect(FakeAppUpdater(named: "staged").scenario == .staged)
        #expect(FakeAppUpdater(named: nil).scenario == .none)
    }
}

struct AppUpdateStateTests {
    @Test func statusTextShowsProgress() {
        #expect(AppUpdateState.downloading(version: "1.2", fraction: 0.42).statusText == "Downloading Ambit 1.2… 42%")
        #expect(AppUpdateState.downloading(version: nil, fraction: nil).statusText == "Downloading update…")
        #expect(AppUpdateState.readyToRestart(version: "1.2").statusText == "Ambit 1.2 is ready to install")
        #expect(AppUpdateState.failed(message: "Offline").statusText == "App update failed: Offline")
    }

    @Test func actionsFollowTheState() {
        let checkable: [AppUpdateState] = [.idle, .upToDate, .failed(message: "x")]
        let busy: [AppUpdateState] = [
            .checking, .downloading(version: nil, fraction: 0.1), .preparing(version: nil, fraction: nil), .installing,
        ]

        for state in checkable {
            #expect(state.canCheck, "\(state)")
            #expect(!state.isBusy, "\(state)")
        }
        for state in busy {
            #expect(!state.canCheck, "\(state)")
            #expect(state.isBusy, "\(state)")
        }
        #expect(!AppUpdateState.unavailable(reason: "x").canCheck)
        #expect(AppUpdateState.readyToRestart(version: nil).canRestartToUpdate)
        #expect(AppUpdateState.downloading(version: nil, fraction: 0.3).progress == 0.3)
    }
}

/// The Sparkle user driver and delegate callbacks, driven directly: no network, no appcast.
@MainActor
struct SparkleAppUpdaterTests {
    private struct Failure: LocalizedError {
        var errorDescription: String? { "The network connection was lost." }
    }

    @Test func aBuildWithoutAFeedKeyIsUnavailable() {
        // The test host is a local build, so SUPublicEDKey is empty.
        let updater = SparkleAppUpdater()

        updater.start()

        guard case .unavailable = updater.state else {
            Issue.record("expected unavailable, got \(updater.state)")
            return
        }
        #expect(!updater.state.canCheck)
    }

    @Test func userInitiatedDownloadReportsProgressAndWaitsForRestart() {
        let updater = SparkleAppUpdater()
        var choice: SPUUserUpdateChoice?

        updater.showUserInitiatedUpdateCheck(cancellation: {})
        #expect(updater.state == .checking)

        updater.showDownloadInitiated(cancellation: {})
        updater.showDownloadDidReceiveExpectedContentLength(200)
        updater.showDownloadDidReceiveData(ofLength: 50)
        #expect(updater.state == .downloading(version: nil, fraction: 0.25))

        updater.showDownloadDidStartExtractingUpdate()
        updater.showExtractionReceivedProgress(0.5)
        #expect(updater.state == .preparing(version: nil, fraction: 0.5))

        updater.showReady(toInstallAndRelaunch: { choice = $0 })
        #expect(updater.state.canRestartToUpdate)
        #expect(choice == nil, "never relaunches on its own")

        updater.installAndRelaunch()
        #expect(choice == .install)
        #expect(updater.state == .installing)
    }

    @Test func aBackgroundDownloadIsStagedForRestart() {
        let updater = SparkleAppUpdater()
        let sparkle = SPUUpdater(
            hostBundle: .main, applicationBundle: .main, userDriver: updater, delegate: updater)
        var installed = 0

        updater.updater(sparkle, willDownloadUpdate: .empty(), with: NSMutableURLRequest())
        #expect(updater.state == .downloading(version: nil, fraction: nil))

        let handled = updater.updater(sparkle, willInstallUpdateOnQuit: .empty()) { installed += 1 }
        #expect(handled)
        #expect(updater.state.canRestartToUpdate)
        #expect(installed == 0)

        // A later background failure must not hide the staged update.
        updater.updater(sparkle, didAbortWithError: Failure())
        #expect(updater.state.canRestartToUpdate)

        updater.installAndRelaunch()
        #expect(installed == 1)
    }

    @Test func failuresKeepTheCurrentVersionAndOfferRetry() {
        let updater = SparkleAppUpdater()
        var acknowledged = false

        updater.showDownloadInitiated(cancellation: {})
        updater.showUpdaterError(Failure()) { acknowledged = true }

        #expect(acknowledged)
        #expect(updater.state == .failed(message: "The network connection was lost."))
        #expect(updater.state.canCheck)
    }

    @Test func noUpdateIsUpToDate() {
        let updater = SparkleAppUpdater()
        var acknowledged = false

        updater.showUserInitiatedUpdateCheck(cancellation: {})
        updater.showUpdateNotFoundWithError(
            NSError(domain: SUSparkleErrorDomain, code: Int(SUError.noUpdateError.rawValue))
        ) { acknowledged = true }

        #expect(acknowledged)
        #expect(updater.state == .upToDate)
        #expect(updater.lastCheckedAt != nil)
    }

    @Test func aNoUpdateAbortIsNotAFailure() {
        let updater = SparkleAppUpdater()
        let sparkle = SPUUpdater(
            hostBundle: .main, applicationBundle: .main, userDriver: updater, delegate: updater)

        updater.updater(
            sparkle, didAbortWithError: NSError(domain: SUSparkleErrorDomain, code: Int(SUError.noUpdateError.rawValue)))

        #expect(updater.state == .upToDate)
    }
}
