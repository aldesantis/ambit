import Foundation
import Testing

@testable import Ambit

@MainActor
struct CatalogUpdatesModelTests {
    private let sandbox: ProjectRegistryTests.Sandbox
    private let engine: FakeEngineService
    private let setup: SetupModel

    private static let installed = "1111111111111111111111111111111111111111"
    private static let latest = "2222222222222222222222222222222222222222"

    init() async throws {
        sandbox = try ProjectRegistryTests.Sandbox()
        engine = FakeEngineService.scenario("catalogUpdates", home: sandbox.home)
        setup = SetupModel(id: .personal, root: sandbox.home, engine: engine, operations: OperationRunner())
        await setup.refresh()
    }

    private var root: String { sandbox.home.path }

    private func model() -> CatalogUpdatesModel {
        CatalogUpdatesModel(setup: setup, store: AppStateStore(directory: sandbox.state))
    }

    private func status(_ name: String, in model: CatalogUpdatesModel) -> CatalogUpdatesModel.Status? {
        model.rows.first { $0.id == name }?.status
    }

    @Test func startsUncheckedAndNeverChecksByItself() {
        let model = model()

        #expect(model.lastCheckedAt == nil)
        #expect(status("team", in: model) == .unchecked)
        #expect(status("docs", in: model) == .unchecked)
        #expect(status("private", in: model) == .unchecked)
        #expect(engine.fixture(root: root).checkCounts.isEmpty)
    }

    @Test func pinnedAndLocalCatalogsAreNeverChecked() async {
        let model = model()

        #expect(status("frozen", in: model) == .pinned(commit: "0123456789abcdef0123456789abcdef01234567"))
        #expect(status("notes", in: model) == .localFiles)
        #expect(model.checkableCatalogs == ["team", "private", "docs"])

        await model.checkAll()
        await model.check(["frozen", "notes"])

        let counts = engine.fixture(root: root).checkCounts
        #expect(counts["frozen"] == nil)
        #expect(counts["notes"] == nil)
        #expect(status("frozen", in: model) == .pinned(commit: "0123456789abcdef0123456789abcdef01234567"))
        #expect(status("notes", in: model) == .localFiles)
    }

    @Test func checksEachCatalogSeparatelySoAFailureHidesNothing() async throws {
        let model = model()

        await model.checkAll()

        #expect(engine.fixture(root: root).checkCounts == ["team": 1, "private": 1, "docs": 1])
        guard case let .updateAvailable(record) = status("team", in: model) else {
            Issue.record("team should have an update")
            return
        }
        #expect(record.installed == Self.installed)
        #expect(record.latest == Self.latest)
        #expect(record.added.map(\.name) == ["triage"])
        guard case .upToDate = status("docs", in: model) else {
            Issue.record("docs should be up to date")
            return
        }
        guard case let .failed(message, _, _) = status("private", in: model) else {
            Issue.record("an unreachable catalog must not be up to date")
            return
        }
        #expect(message.contains("acme/private"))
        #expect(model.lastCheckedAt != nil)
        #expect(model.availableUpdates == [ReviewedRevision(catalog: "team", commit: Self.latest)])
    }

    @Test func checkResultsAndTimeArePersistedPerSetup() async throws {
        let first = model()
        await first.checkAll()

        let reloaded = model()

        #expect(reloaded.lastCheckedAt == first.lastCheckedAt)
        #expect(reloaded.rows == first.rows)
        let state = AppStateStore(directory: sandbox.state).state
        #expect(state.catalogChecks[root]?.results.keys.sorted() == ["docs", "private", "team"])
    }

    @Test func aFailedCheckKeepsTheLastKnownInstallation() async throws {
        let model = model()
        await model.check(["team"])
        engine.updateFixture(root: root) {
            $0.catalogChecks["team"] = .failure(
                .network(message: "Could not reach github.com.", detail: [], kind: .offline))
        }

        await model.check(["team"])

        guard case let .failed(_, lastKnown, _) = status("team", in: model) else {
            Issue.record("the retry should have failed")
            return
        }
        #expect(lastKnown == Self.installed)
    }

    @Test func updatingRequiresResolvingTheDraftFirst() async throws {
        let model = model()
        await model.checkAll()
        try setup.stage([.setHarnesses(harnesses: ["claude", "codex"])])

        await model.reviewUpdate()

        #expect(model.isBlockedByDraft)
        #expect(model.review == nil)
        #expect(engine.fixture(root: root).reviewedRevisions.isEmpty)

        setup.discardChanges()
        await model.reviewUpdate()

        #expect(!model.isBlockedByDraft)
        #expect(model.review != nil)
    }

    @Test func applyInstallsExactlyTheReviewedCommits() async throws {
        let model = model()
        await model.checkAll()
        await model.reviewUpdate(["team"])
        let review = try #require(model.review)
        guard case let .ready(summary, refreshed) = review.phase else {
            Issue.record("the review should be ready")
            return
        }
        #expect(!refreshed)
        #expect(summary.diff.added == [ItemRef(kind: .skill, catalog: "team", name: "triage")])

        // The remote branch moves after the review.
        engine.updateFixture(root: root) {
            $0.catalogChecks["team"] = .outdated(
                installed: Self.installed, latest: "3333333333333333333333333333333333333333", added: [])
        }
        await review.apply()

        #expect(review.didInstall)
        let fixture = engine.fixture(root: root)
        #expect(fixture.reviewedRevisions == [[ReviewedRevision(catalog: "team", commit: Self.latest)]])
        #expect(fixture.appliedRevisions == [[ReviewedRevision(catalog: "team", commit: Self.latest)]])
        guard case .upToDate = status("team", in: model) else {
            Issue.record("team should be up to date after the update")
            return
        }
    }

    @Test func aStaleReviewIsRefreshedAndAskedAgain() async throws {
        let model = model()
        await model.checkAll()
        await model.reviewUpdate()
        let review = try #require(model.review)
        guard case let .valid(_, _, text, _) = engine.fixture(root: root).config else {
            Issue.record("the fixture should have a config")
            return
        }
        let edited = try FakeEngineService.validConfig("# Edited elsewhere.\n" + text, root: root)
        engine.updateFixture(root: root) { $0.config = edited }

        await review.apply()

        guard case .ready(_, refreshed: true) = review.phase else {
            Issue.record("the plan should be refreshed, not applied: \(review.phase)")
            return
        }
        let fixture = engine.fixture(root: root)
        #expect(fixture.saveCount == 0)
        #expect(fixture.appliedRevisions.isEmpty)
        #expect(fixture.reviewedRevisions.count == 2)

        await review.apply()

        #expect(review.didInstall)
        #expect(engine.fixture(root: root).appliedRevisions == [[ReviewedRevision(catalog: "team", commit: Self.latest)]])
    }

    @Test func anUnappliableReviewDoesNotApply() async throws {
        let model = model()
        await model.checkAll()
        engine.updateFixture(root: root) {
            $0.blockers = [.resolution(error: .resolution(message: "team/review no longer exists.", detail: []))]
        }

        await model.reviewUpdate()
        let review = try #require(model.review)
        await review.apply()

        guard case let .ready(summary, _) = review.phase else {
            Issue.record("the review should stay open")
            return
        }
        #expect(!summary.canApply)
        #expect(engine.fixture(root: root).applyCount == 0)
    }

    @Test func aFailedUpdateKeepsTheCheckResult() async throws {
        let model = model()
        await model.checkAll()
        engine.updateFixture(root: root) {
            $0.applyFailure = .beforeSave(.network(message: "Offline.", detail: [], kind: .offline))
        }

        await model.reviewUpdate()
        let review = try #require(model.review)
        await review.apply()

        #expect(review.phase == .finished(.saveFailed(.network(message: "Offline.", detail: [], kind: .offline))))
        guard case .updateAvailable = status("team", in: model) else {
            Issue.record("team should still show its update")
            return
        }
    }

    @Test func modelIsKeptPerSetup() {
        let store = AppStateStore(directory: sandbox.state)

        #expect(setup.catalogUpdates(store: store) === setup.catalogUpdates(store: store))
    }
}
