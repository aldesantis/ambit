import AppKit
import Foundation
import Testing

@testable import Ambit

@MainActor
final class FakePendingChanges: PendingChangesGuard {
    var hasPendingChanges = false
    var isApplying = false
    /// What the user answers: true for Apply/Discard, false for Cancel.
    var answer = true
    private(set) var asked: [LeaveReason] = []

    func resolvePendingChanges(for reason: LeaveReason) async -> Bool {
        asked.append(reason)
        if answer {
            hasPendingChanges = false
        }
        return answer
    }
}

@MainActor
struct AppLifecycleTests {
    private let sandbox: ProjectRegistryTests.Sandbox
    private let updater: FakeAppUpdater
    private let pending = FakePendingChanges()
    private let lifecycle: AppLifecycle
    private let policies: PolicyLog

    @MainActor final class PolicyLog {
        var values: [NSApplication.ActivationPolicy] = []
    }

    init() throws {
        sandbox = try ProjectRegistryTests.Sandbox()
        updater = FakeAppUpdater(scenario: .staged)
        lifecycle = AppLifecycle(
            updater: updater,
            loginItem: LoginItemService(
                control: InMemoryLoginItemControl(), store: AppStateStore(directory: sandbox.state)),
            pendingChanges: pending)
        let policies = PolicyLog()
        self.policies = policies
        lifecycle.setActivationPolicy = { policies.values.append($0) }
        updater.start()
    }

    // MARK: Restart to update

    @Test func restartInstallsWhenNothingIsPending() async {
        await lifecycle.restartToUpdate()

        #expect(updater.installCount == 1)
        #expect(pending.asked.isEmpty)
    }

    @Test func restartAsksAboutDraftsAndKeepsThemOnCancel() async {
        pending.hasPendingChanges = true
        pending.answer = false

        await lifecycle.restartToUpdate()

        #expect(pending.asked == [.restartToUpdate])
        #expect(updater.installCount == 0)
        #expect(pending.hasPendingChanges, "the draft is never discarded for an update")
        #expect(updater.state.canRestartToUpdate)
        #expect(lifecycle.leaving == nil)
    }

    @Test func restartContinuesAfterTheDraftIsResolved() async {
        pending.hasPendingChanges = true
        pending.answer = true

        await lifecycle.restartToUpdate()

        #expect(pending.asked == [.restartToUpdate])
        #expect(updater.installCount == 1)
    }

    @Test func restartWaitsForAnInFlightApply() async throws {
        pending.isApplying = true

        let restart = Task { await lifecycle.restartToUpdate() }
        try await Task.sleep(for: .milliseconds(300))

        #expect(updater.installCount == 0)
        #expect(lifecycle.isWaitingForOperation)
        #expect(lifecycle.leaving == .restartToUpdate)

        pending.isApplying = false
        await restart.value

        #expect(updater.installCount == 1)
        #expect(!lifecycle.isWaitingForOperation)
    }

    @Test func restartNeedsAStagedUpdate() async {
        let idle = FakeAppUpdater(scenario: .none)
        idle.start()
        let lifecycle = AppLifecycle(
            updater: idle,
            loginItem: LoginItemService(
                control: InMemoryLoginItemControl(), store: AppStateStore(directory: sandbox.state)),
            pendingChanges: pending)

        await lifecycle.restartToUpdate()

        #expect(idle.installCount == 0)
    }

    // MARK: Quit

    @Test func quitTerminatesAtOnceWhenNothingIsPending() {
        #expect(lifecycle.terminationReply { _ in Issue.record("no deferred reply expected") } == .terminateNow)
    }

    @Test func quitWithADraftAsksAndCancelKeepsRunning() async throws {
        pending.hasPendingChanges = true
        pending.answer = false
        let reply = ReplyBox()

        #expect(lifecycle.terminationReply { reply.value = $0 } == .terminateLater)
        try await waitUntil { reply.value != nil }

        #expect(reply.value == false)
        #expect(pending.asked == [.quit])
        #expect(pending.hasPendingChanges)
    }

    @Test func quitDuringApplyWaitsThenTerminates() async throws {
        pending.isApplying = true
        let reply = ReplyBox()

        #expect(lifecycle.terminationReply { reply.value = $0 } == .terminateLater)
        try await Task.sleep(for: .milliseconds(250))
        #expect(reply.value == nil)

        pending.isApplying = false
        try await waitUntil { reply.value != nil }

        #expect(reply.value == true)
        // The termination AppKit sends after approval goes straight through.
        #expect(lifecycle.terminationReply { _ in } == .terminateNow)
    }

    // MARK: Window and Dock

    @Test func closingTheWindowWithADraftAsksFirst() async throws {
        pending.hasPendingChanges = true
        let closed = ReplyBox()

        #expect(!lifecycle.shouldCloseMainWindow { closed.value = true })
        try await waitUntil { closed.value != nil }

        #expect(pending.asked == [.closeWindow])
    }

    @Test func closingTheWindowWithoutADraftIsImmediate() {
        #expect(lifecycle.shouldCloseMainWindow { Issue.record("closes directly") })
    }

    @Test func dockIconFollowsTheWindows() {
        lifecycle.windowsChanged(hasVisibleWindow: true)
        lifecycle.windowsChanged(hasVisibleWindow: false)

        #expect(policies.values == [.regular, .accessory])
    }

    @Test func loginLaunchStaysInTheMenuBar() {
        let updater = FakeAppUpdater(scenario: .none)
        let control = InMemoryLoginItemControl()
        let lifecycle = AppLifecycle(
            updater: updater,
            loginItem: LoginItemService(control: control, store: AppStateStore(directory: sandbox.state)))
        let policies = PolicyLog()
        lifecycle.setActivationPolicy = { policies.values.append($0) }

        lifecycle.didFinishLaunching(isLoginLaunch: true)

        #expect(policies.values == [.accessory])
        #expect(updater.startCount == 1)
        #expect(control.registerCount == 1)
    }

    // MARK: Helpers

    @MainActor final class ReplyBox {
        var value: Bool?
    }

    private func waitUntil(_ condition: () -> Bool) async throws {
        for _ in 0..<100 where !condition() {
            try await Task.sleep(for: .milliseconds(20))
        }
        #expect(condition())
    }
}
