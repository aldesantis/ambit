import Foundation
import Testing

@testable import Ambit

/// Waits until `condition` holds, failing the test after about five seconds.
@MainActor
func eventually(_ condition: () -> Bool, sourceLocation: SourceLocation = #_sourceLocation) async throws {
    for _ in 0..<500 {
        if condition() {
            return
        }
        try await Task.sleep(for: .milliseconds(10))
    }
    Issue.record("The condition never held.", sourceLocation: sourceLocation)
}

@MainActor
struct SetupModelTests {
    private let root = "/fake/setup"
    private let engine = FakeEngineService()
    private let savedText = "version: 1\nharnesses:\n  - claude\ncatalogs: []\nrequires: []\n"

    private func setup(saved: String? = nil) async throws -> SetupModel {
        if let saved {
            engine.setFixture(.init(config: try FakeEngineService.validConfig(saved, root: root)), root: root)
        }
        let setup = SetupModel(
            id: .personal, root: URL(fileURLWithPath: root), engine: engine, operations: OperationRunner())
        await setup.refresh()
        return setup
    }

    private var fixture: FakeEngineService.Fixture { engine.fixture(root: root) }

    private func reviewed(_ setup: SetupModel) async throws -> ReviewModel {
        await setup.startReview()
        let review = try #require(setup.review)
        guard case .ready = review.phase else {
            Issue.record("Expected a ready review, got \(review.phase)")
            return review
        }
        return review
    }

    // MARK: Drafts

    @Test func aMissingConfigIsUnconfigured() async throws {
        let setup = try await setup()

        #expect(setup.snapshot?.config == .missing)
        #expect(setup.badge == .unconfigured)
        #expect(!setup.hasPendingChanges)
        #expect(setup.configSummary == nil)
    }

    @Test func aNewSetupDraftIsPendingUntilDiscarded() async throws {
        let setup = try await setup()

        try setup.startNewSetup(harnesses: ["claude", "codex"])

        #expect(setup.hasPendingChanges)
        #expect(setup.badge == .pendingChanges)
        #expect(setup.configSummary?.harnesses == ["claude", "codex"])
        #expect(setup.fileName == "ambit.yml")

        setup.discardChanges()

        #expect(!setup.hasPendingChanges)
        #expect(setup.newSetupStep == .tools)
        #expect(fixture.config == .missing)
        #expect(fixture.applyCount == 0)
    }

    @Test func changingToolsKeepsLaterEdits() async throws {
        let setup = try await setup()
        try setup.startNewSetup(harnesses: ["claude"])
        try setup.stage([.addCatalog(name: "team", source: "./team", gitRef: nil)])

        try setup.startNewSetup(harnesses: ["cursor"])

        #expect(setup.configSummary?.harnesses == ["cursor"])
        #expect(setup.configSummary?.catalogs.map(\.name) == ["team"])
    }

    @Test func stagingEditsAnExistingConfigInMemory() async throws {
        let setup = try await setup(saved: savedText)
        let entry = SelectionEntry(kind: .skill, catalog: "team", pattern: "review", isRule: false)

        try setup.stage([.addCatalog(name: "team", source: "./team", gitRef: nil), .addEntry(entry)])

        #expect(setup.hasPendingChanges)
        #expect(setup.configSummary?.requires == [entry])
        #expect(setup.draftText?.contains("skill: team/review") == true)
        #expect(fixture.config == (try FakeEngineService.validConfig(savedText, root: root)))

        setup.discardChanges()

        #expect(setup.draftText == nil)
        #expect(setup.configSummary?.requires == [])
        #expect(fixture.saveCount == 0)
    }

    @Test func editsThatCancelOutAreNotPending() async throws {
        let setup = try await setup(saved: savedText)

        try setup.stage([.setHarnesses(harnesses: ["codex"])])
        try setup.stage([.setHarnesses(harnesses: ["claude"])])

        #expect(!setup.hasPendingChanges)
        #expect(setup.draftText == nil)
    }

    @Test func aFailedEditLeavesTheDraftUnchanged() async throws {
        let setup = try await setup(saved: savedText)
        try setup.stage([.addCatalog(name: "team", source: "./team", gitRef: nil)])
        let before = setup.draftText

        #expect(throws: EngineError.self) {
            try setup.stage([.addCatalog(name: "team", source: "./other", gitRef: nil)])
        }
        #expect(setup.draftText == before)
    }

    @Test func anUnconfiguredSetupNeedsTheNewSetupFlowToEdit() async throws {
        let setup = try await setup()

        #expect(throws: EngineError.self) {
            try setup.stage([.setHarnesses(harnesses: ["claude"])])
        }
    }

    // MARK: Review and apply

    @Test func createsAnEmptySetupThroughReviewAndApply() async throws {
        let setup = try await setup()
        try setup.startNewSetup(harnesses: ["claude"])

        let review = try await reviewed(setup)
        guard case let .ready(summary) = review.phase else {
            return
        }
        #expect(summary.config.harnessesAdded == ["claude"])
        #expect(summary.writes.contains { $0.path.hasSuffix("/ambit.yml") })
        #expect(fixture.saveCount == 0)

        await review.apply()

        #expect(review.didInstall)
        #expect(fixture.saveCount == 1)
        #expect(!setup.hasPendingChanges)
        #expect(setup.badge == .installed)
        guard case let .valid(_, fileName, _, saved) = setup.snapshot?.config else {
            Issue.record("Expected a saved config")
            return
        }
        #expect(fileName == "ambit.yml")
        #expect(saved.harnesses == ["claude"])

        setup.closeReview()
        #expect(setup.review == nil)
    }

    @Test func cancelingAReviewWritesNothing() async throws {
        let setup = try await setup()
        try setup.startNewSetup(harnesses: ["claude"])
        _ = try await reviewed(setup)

        setup.closeReview()

        #expect(setup.review == nil)
        #expect(setup.hasPendingChanges)
        #expect(fixture.applyCount == 0)
        #expect(fixture.config == .missing)
    }

    @Test func blockersPreventApply() async throws {
        let setup = try await setup(saved: savedText)
        let conflict = OwnershipConflict(
            path: "/fake/setup/.claude/skills/review", key: nil, message: "Not managed by Ambit.", detail: [])
        engine.updateFixture(root: root) { $0.blockers = [.ownership(conflict: conflict)] }
        try setup.stage([.setHarnesses(harnesses: ["codex"])])

        let review = try await reviewed(setup)
        guard case let .ready(summary) = review.phase else {
            return
        }
        #expect(!summary.canApply)

        await review.apply()

        #expect(review.phase == .ready(summary))
        #expect(fixture.applyCount == 0)
    }

    @Test func aStaleReviewIsNotAppliedAndAsksAboutTheFile() async throws {
        let setup = try await setup(saved: savedText)
        try setup.stage([.setHarnesses(harnesses: ["codex"])])
        let review = try await reviewed(setup)
        let external = savedText.replacingOccurrences(of: "claude", with: "cursor")
        let changed = try FakeEngineService.validConfig(external, root: root)
        engine.updateFixture(root: root) { $0.config = changed }

        await review.apply()

        guard case .finished(.stale) = review.phase else {
            Issue.record("Expected a stale review, got \(review.phase)")
            return
        }
        #expect(fixture.saveCount == 0)
        #expect(setup.hasPendingChanges)

        #expect(await review.reviewAgain() == false)
        #expect(setup.externalChange?.config == changed)

        setup.closeReview()
        setup.reloadDiscardingDraft()

        #expect(!setup.hasPendingChanges)
        #expect(setup.snapshot?.config == changed)
    }

    @Test func aStaleReviewOfACleanSetupReviewsAgain() async throws {
        let setup = try await setup()
        try setup.startNewSetup(harnesses: ["claude"])
        let review = try await reviewed(setup)
        // The engine's fingerprint covers more than the config, such as the lock and the
        // installed files, so a stale review can happen with the config file unchanged.
        engine.updateFixture(root: root) { $0.applyFailure = .beforeSave(.staleReview(message: "Stale", detail: [])) }

        await review.apply()
        #expect(review.phase == .finished(.stale(.staleReview(message: "Stale", detail: []))))

        engine.updateFixture(root: root) { $0.applyFailure = nil }
        #expect(await review.reviewAgain())
        guard case .ready = review.phase else {
            Issue.record("Expected a fresh review")
            return
        }
        await review.apply()
        #expect(review.didInstall)
    }

    @Test func aFailedSaveKeepsTheDraft() async throws {
        let setup = try await setup(saved: savedText)
        try setup.stage([.setHarnesses(harnesses: ["codex"])])
        let draft = setup.draftText
        let busy = EngineError.busy(message: "Another operation is running.", detail: [])
        engine.updateFixture(root: root) { $0.applyFailure = .beforeSave(busy) }
        let review = try await reviewed(setup)

        await review.apply()

        #expect(review.phase == .finished(.saveFailed(busy)))
        #expect(setup.draftText == draft)
        #expect(setup.installFailure == nil)
        #expect(fixture.saveCount == 0)
    }

    @Test func aFailedInstallKeepsTheSavedConfigAndOffersRetry() async throws {
        let setup = try await setup(saved: savedText)
        try setup.stage([.setHarnesses(harnesses: ["codex"])])
        let failure = EngineError.ownershipConflict(message: "Not managed by Ambit.", detail: [], path: "/x")
        engine.updateFixture(root: root) { $0.applyFailure = .afterSave(failure) }
        let review = try await reviewed(setup)

        await review.apply()

        #expect(review.phase == .finished(.notFullyInstalled(saved: true, error: failure)))
        #expect(!setup.hasPendingChanges)
        #expect(setup.installFailure == failure)
        #expect(setup.badge == .notFullyInstalled)
        #expect(setup.configSummary?.harnesses == ["codex"])

        setup.closeReview()
        engine.updateFixture(root: root) { $0.retryFailure = failure }
        await setup.retryInstall()
        #expect(setup.installFailure == failure)

        setup.closeReview()
        engine.updateFixture(root: root) { $0.retryFailure = nil }
        await setup.retryInstall()
        #expect(setup.installFailure == nil)
        #expect(setup.badge == .installed)
    }

    @Test func aRetryKeepsANewerDraft() async throws {
        let setup = try await setup(saved: savedText)
        try setup.stage([.setHarnesses(harnesses: ["cursor"])])

        await setup.retryInstall()

        #expect(setup.review?.didInstall == true)
        #expect(setup.configSummary?.harnesses == ["cursor"])
    }

    @Test func applyCanBeCanceledBeforeItWrites() async throws {
        let setup = try await setup()
        try setup.startNewSetup(harnesses: ["claude"])
        let review = try await reviewed(setup)
        engine.updateFixture(root: root) { $0.delay = .seconds(5) }

        let applying = Task { await review.apply() }
        try await eventually { review.operation?.progress?.stage == .checkingOwnership }
        #expect(review.canCancel)
        review.cancel()
        await applying.value

        #expect(review.phase == .finished(.canceled))
        #expect(fixture.saveCount == 0)
        #expect(setup.hasPendingChanges)
    }

    @Test func aReviewCanBeCanceled() async throws {
        let setup = try await setup()
        try setup.startNewSetup(harnesses: ["claude"])
        engine.updateFixture(root: root) { $0.delay = .seconds(5) }

        let reviewing = Task { await setup.startReview() }
        try await eventually { setup.review?.operation?.progress?.stage == .resolving }
        setup.closeReview()
        await reviewing.value

        #expect(setup.review == nil)
        #expect(setup.hasPendingChanges)
        #expect(!setup.operations.isBusy)
    }

    // MARK: External changes

    @Test func aCleanSetupTakesExternalChanges() async throws {
        let setup = try await setup(saved: savedText)
        let changed = try FakeEngineService.validConfig(savedText + "# edited\n", root: root)
        engine.updateFixture(root: root) { $0.config = changed }

        await setup.refresh()

        #expect(setup.snapshot?.config == changed)
        #expect(setup.externalChange == nil)
    }

    @Test func aDirtySetupAsksAboutExternalChangesOnce() async throws {
        let setup = try await setup(saved: savedText)
        try setup.stage([.setHarnesses(harnesses: ["codex"])])
        let changed = try FakeEngineService.validConfig(savedText + "# edited\n", root: root)
        engine.updateFixture(root: root) { $0.config = changed }

        await setup.refresh()
        #expect(setup.externalChange?.config == changed)

        setup.keepDraftAfterExternalChange()
        #expect(setup.isDraftOutdated)
        #expect(setup.configSummary?.harnesses == ["codex"])

        await setup.refresh()
        #expect(setup.externalChange == nil)

        await setup.startReview()
        #expect(setup.review == nil)
        #expect(setup.externalChange?.config == changed)
        #expect(fixture.applyCount == 0)
    }

    @Test func aNewSetupDraftConflictsWithAConfigCreatedElsewhere() async throws {
        let setup = try await setup()
        try setup.startNewSetup(harnesses: ["claude"])
        let created = try FakeEngineService.validConfig(savedText, root: root)
        engine.updateFixture(root: root) { $0.config = created }

        await setup.startReview()

        #expect(setup.review == nil)
        #expect(setup.externalChange != nil)
    }

    @Test func invalidAndAmbiguousConfigsAreShownAsTheyAre() async throws {
        let problem = ConfigProblem(message: "Unknown field.", detail: [], line: 3)
        engine.setFixture(.init(config: .invalid(path: "/fake/setup/ambit.yml", fileName: "ambit.yml", problem: problem)), root: root)
        let setup = try await setup()

        #expect(setup.badge == .error)
        #expect(throws: EngineError.self) {
            try setup.stage([.setHarnesses(harnesses: ["claude"])])
        }

        engine.setFixture(.init(config: .ambiguous(files: ["a", "b"], problem: problem)), root: root)
        await setup.refresh()
        #expect(setup.badge == .error)
    }
}

@MainActor
struct OperationRunnerTests {
    @Test func runsOneOperationAtATime() async throws {
        @MainActor final class Log {
            var lines: [String] = []
        }
        let runner = OperationRunner()
        let log = Log()

        let first = Task {
            try await runner.run("first") { _ in
                log.lines.append("first start")
                try await Task.sleep(for: .milliseconds(50))
                log.lines.append("first end")
            }
        }
        try await eventually { runner.current?.title == "first" }
        let second = Task {
            try await runner.run("second") { _ in
                log.lines.append("second start")
                log.lines.append("second end")
            }
        }
        try await first.value
        try await second.value

        #expect(log.lines == ["first start", "first end", "second start", "second end"])
        #expect(!runner.isBusy)
    }

    @Test func stopsBeingCancellableOnceItWrites() async throws {
        let runner = OperationRunner()

        try await runner.run("apply") { progress in
            #expect(runner.current?.isCancellable == true)
            progress(ProgressEvent(stage: .savingConfig, subject: "", current: 0, total: 0))
            #expect(runner.current?.isCancellable == false)
        }
    }
}
