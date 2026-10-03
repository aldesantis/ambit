import Foundation
import Testing

@testable import Ambit

@MainActor
struct HealthModelTests {
    private let root = "/tmp/ambit-health-tests"
    private let review = ItemRef(kind: .skill, catalog: "team", name: "review")
    private let github = ItemRef(kind: .mcp, catalog: "team", name: "github")

    private func artifact(_ path: String, _ state: ArtifactState) -> StatusArtifact {
        StatusArtifact(path: path, kind: "skill", state: state, detail: nil)
    }

    private func item(_ ref: ItemRef, _ states: ArtifactState...) -> ItemStatus {
        let artifacts = states.enumerated().map { artifact("\(ref.name)/\($0.offset)", $0.element) }
        return ItemStatus(item: ref, state: states.max() ?? .ok, artifacts: artifacts)
    }

    private func expects(_ variable: String, by line: String, subject: ItemRef? = nil) -> Finding {
        Finding(
            check: "expects", severity: .error, message: "unset environment variable \"\(variable)\"",
            detail: [line, "set \(variable) in the environment the agent runs in"], subject: subject, harness: nil)
    }

    private func setup(_ change: (inout FakeEngineService.Fixture) -> Void = { _ in }) async throws
        -> (SetupModel, FakeEngineService)
    {
        let engine = FakeEngineService()
        let text = engine.newConfigText(harnesses: ["claude", "codex"])
        var fixture = FakeEngineService.Fixture(config: try FakeEngineService.validConfig(text, root: root))
        change(&fixture)
        engine.setFixture(fixture, root: root)
        let setup = SetupModel(
            id: .project(path: root), root: URL(fileURLWithPath: root), engine: engine, operations: OperationRunner())
        await setup.refresh()
        return (setup, engine)
    }

    // MARK: Status categories

    @Test func categoriesKeepInstallationAndHealthDistinct() {
        let findings = [expects("GITHUB_TOKEN", by: "MCP server \"github\" expects it")]

        #expect(HealthModel.category(for: item(review, .ok), findings: findings) == .installed)
        #expect(HealthModel.category(for: item(github, .ok), findings: findings) == .setupRequired)
        #expect(HealthModel.category(for: item(review, .missing, .missing), findings: []) == .selectedNotInstalled)
        #expect(HealthModel.category(for: item(review, .ok, .missing), findings: []) == .notFullyInstalled)
        #expect(HealthModel.category(for: item(review, .modified), findings: []) == .drifted)
        #expect(HealthModel.category(for: item(review, .unowned, .ok), findings: []) == .ownershipProblem)
        #expect(ItemCategory.setupRequired.title == "Installed; setup required")
    }

    @Test func aFindingSubjectLinksItToItsItem() {
        let finding = expects("API_KEY", by: "skill \"other\" expects it", subject: review)

        #expect(HealthModel.category(for: item(review, .ok), findings: [finding]) == .setupRequired)
        #expect(HealthModel.category(for: item(github, .ok), findings: [finding]) == .installed)
    }

    @Test func refreshStatusReadsLocalStatusOnly() async throws {
        let (setup, engine) = try await setup { $0.status = SetupStatus(items: [self.item(self.review, .ok)], artifacts: []) }

        await setup.health.refreshStatus()

        #expect(engine.fixture(root: root).statusReads == 1)
        #expect(engine.fixture(root: root).healthChecks == 0)
        #expect(setup.health.items.map(\.category) == [.installed])
        #expect(setup.healthSummary?.level == .installed)
    }

    @Test func checkHealthReportsSetupRequired() async throws {
        let (setup, _) = try await setup { fixture in
            let items = [self.item(self.review, .ok), self.item(self.github, .ok)]
            fixture.status = SetupStatus(items: items, artifacts: [])
            fixture.health = HealthReport(
                checks: [], findings: [self.expects("GITHUB_TOKEN", by: "MCP server \"github\" expects it")],
                items: items)
        }

        await setup.health.checkHealth()

        #expect(setup.health.items.map(\.category) == [.installed, .setupRequired])
        #expect(setup.health.items[1].issues.first?.kind == .missingPrerequisite)
        #expect(setup.healthSummary?.level == .setupRequired)
        #expect(setup.healthSummary?.title == "Installed; setup required")
    }

    @Test func summaryTakesTheWorstLevel() {
        let drifted = ItemHealth(status: item(review, .modified), category: .drifted, issues: [])
        let blocked = ItemHealth(status: item(github, .unowned), category: .ownershipProblem, issues: [])

        #expect(HealthModel.summary(items: [drifted], findings: [], artifacts: []).level == .notFullyInstalled)
        #expect(HealthModel.summary(items: [drifted, blocked], findings: [], artifacts: []).level == .needsAttention)
        #expect(HealthModel.summary(items: [], findings: [], artifacts: []).level == .installed)
    }

    @Test func aSetupWithoutConfigHasNoSummary() async throws {
        let engine = FakeEngineService()
        let setup = SetupModel(
            id: .project(path: root), root: URL(fileURLWithPath: root), engine: engine, operations: OperationRunner())

        await setup.health.refreshStatus()

        #expect(setup.healthSummary == nil)
        #expect(engine.fixture(root: root).statusReads == 0)
        #expect(setup.health.reapplyBlockedReason != nil)
    }

    @Test func aFailedReadIsReported() async throws {
        let failure = EngineError.io(message: "Cannot read the setup.", detail: [], path: nil)
        let (setup, _) = try await setup { $0.statusFailure = failure }

        await setup.health.refreshStatus()

        #expect(setup.health.error == failure)
    }

    // MARK: Finding mapping

    @Test func mapsEachCheckToAPlainExplanation() {
        let tools = FakeEngineService().supportedAgentTools()

        let declared = HealthModel.issue(
            for: expects("API_KEY", by: "skill \"review\" expects it", subject: review), tools: tools)
        #expect(declared.kind == .missingPrerequisite)
        #expect(declared.title == "API_KEY is not set")
        #expect(declared.affected == "Skill review from team")
        #expect(declared.nextStep.contains("does not ask for or store"))

        let reference = HealthModel.issue(
            for: expects("TOKEN", by: "\"mcpServers.github\" in .mcp.json references it, for the harness to expand at spawn"),
            tools: tools)
        #expect(reference.kind == .unresolvedReference)
        #expect(reference.affected == "mcpServers.github")

        let drift = HealthModel.issue(
            for: Finding(
                check: "drift", severity: .error, message: ".claude/skills/review is modified",
                detail: ["run `ambit install`"], subject: nil, harness: nil), tools: tools)
        #expect(drift.kind == .drift)
        #expect(drift.affected == ".claude/skills/review")
        #expect(drift.action == .reviewAndReapply)
        #expect(!drift.nextStep.contains("ambit install"))

        let ownership = HealthModel.issue(
            for: Finding(
                check: "ownership", severity: .error, message: "ambit does not own .mcp.json", detail: [],
                subject: nil, harness: nil), tools: tools)
        #expect(ownership.kind == .ownership)
        #expect(ownership.affected == ".mcp.json")
        #expect(ownership.explanation.contains("never overwrites"))

        let limitation = HealthModel.issue(
            for: Finding(
                check: "harness", severity: .warning, message: "codex runs hooks only with a flag set",
                detail: ["set the flag in your own codex config to have them run"], subject: nil, harness: "codex"),
            tools: tools)
        #expect(limitation.kind == .toolLimitation)
        #expect(limitation.affected == "Codex")
        #expect(limitation.nextStep == "set the flag in your own codex config to have them run")

        let lock = HealthModel.issue(
            for: Finding(check: "lock", severity: .error, message: "ambit.lock is out of date", detail: [], subject: nil, harness: nil),
            tools: tools)
        #expect(lock.kind == .lockOutdated)
        #expect(lock.technical == ["ambit.lock is out of date"])
    }

    @Test func listsLimitationsOfSelectedToolsOnly() async throws {
        let (setup, _) = try await setup()

        #expect(setup.health.toolLimitations.map(\.id) == ["codex"])
    }

    // MARK: Repair

    @Test func reapplyReviewsTheSavedConfiguration() async throws {
        let (setup, _) = try await setup()

        await setup.health.reviewSavedConfiguration()

        let review = try #require(setup.review)
        guard case let .ready(summary) = review.phase else {
            Issue.record("The review did not finish: \(review.phase)")
            return
        }
        #expect(summary.config == ConfigChanges())
        setup.closeReview()
    }

    @Test func reapplyWaitsForPendingChanges() async throws {
        let (setup, _) = try await setup()
        setup.agentTools.setSelected("cursor", true)

        await setup.health.reviewSavedConfiguration()

        #expect(setup.review == nil)
        #expect(setup.health.reapplyBlockedReason?.contains("pending changes") == true)
    }
}
