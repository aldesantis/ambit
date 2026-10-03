// A scripted updater for unit tests and UI tests (`AMBIT_TEST_UPDATER=<scenario>`). Transitions
// happen synchronously so tests can assert on them without waiting.

import Foundation
import Observation

@MainActor
@Observable
final class FakeAppUpdater: AppUpdater {
    enum Scenario: String, Sendable, CaseIterable {
        /// Every check finds nothing.
        case none
        /// A check finds 99.0 and downloads it.
        case available
        /// The first download fails; a retry succeeds.
        case downloadFailed
        /// An update was already downloaded before this launch.
        case staged
    }

    static let version = "99.0"

    let scenario: Scenario
    private(set) var state: AppUpdateState = .idle
    private(set) var lastCheckedAt: Date?

    /// Observable counters for assertions.
    private(set) var startCount = 0
    private(set) var checkCount = 0
    private(set) var installCount = 0

    private var failuresLeft: Int

    init(scenario: Scenario = .none) {
        self.scenario = scenario
        failuresLeft = scenario == .downloadFailed ? 1 : 0
    }

    convenience init(named name: String?) {
        self.init(scenario: name.flatMap(Scenario.init(rawValue:)) ?? .none)
    }

    func start() {
        startCount += 1
        if scenario == .staged {
            state = .readyToRestart(version: Self.version)
            return
        }
        check()
    }

    func checkForUpdates() {
        guard state.canCheck else {
            return
        }
        check()
    }

    func installAndRelaunch() {
        guard state.canRestartToUpdate else {
            return
        }
        installCount += 1
        state = .installing
    }

    private func check() {
        checkCount += 1
        state = .checking
        lastCheckedAt = .now

        switch scenario {
        case .none:
            state = .upToDate
        case .available, .downloadFailed, .staged:
            state = .downloading(version: Self.version, fraction: 0.5)
            if failuresLeft > 0 {
                failuresLeft -= 1
                state = .failed(message: String(localized: "The update could not be downloaded."))
            } else {
                state = .readyToRestart(version: Self.version)
            }
        }
    }
}
