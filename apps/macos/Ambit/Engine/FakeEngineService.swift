// An in-memory engine for previews, unit tests of UI state, and the app until the UniFFI-backed
// `LiveEngineService` exists. Setups come from `fixtures`; a root without a fixture has no config.
// Nothing here writes to disk. `canonicalPath` is the one call that reads the real filesystem,
// because project registration needs real folders.

import Foundation
import Synchronization

final class FakeEngineService: EngineService {
    struct Fixture: Sendable {
        var config: ConfigState = .missing
        var catalogs: [CatalogLoadState] = []
        var items: [BrowseItem] = []
    }

    private let state: Mutex<[String: Fixture]>
    private let token = Mutex<String?>(nil)

    init(fixtures: [String: Fixture] = [:]) {
        state = Mutex(fixtures)
    }

    var gitHubToken: String? { token.withLock { $0 } }

    func setFixture(_ fixture: Fixture, root: String) {
        state.withLock { $0[root] = fixture }
    }

    func fixture(root: String) -> Fixture {
        state.withLock { $0[root] ?? Fixture() }
    }

    func setGitHubToken(_ token: String?) async {
        self.token.withLock { $0 = token }
    }

    func openSetup(root: String) -> any SetupSessionService {
        FakeSetupSession(root: root, engine: self)
    }

    func canonicalPath(_ path: String) async throws -> String {
        guard let resolved = realpath(path, nil) else {
            throw EngineError.config(
                message: String(localized: "The folder \(path) does not exist."), detail: [], path: path, line: nil)
        }
        defer { free(resolved) }
        return String(cString: resolved)
    }

    func newConfigText(harnesses: [String]) -> String {
        let list = harnesses.map { "  - \($0)\n" }.joined()
        return "version: 1\nharnesses:\n\(list)catalogs: []\nrequires: []\n"
    }

    func editConfig(text: String, fileName: String, edits: [ConfigEdit]) throws -> EditedConfig {
        throw Self.unsupported
    }

    func parseConfig(text: String, fileName: String) throws -> ConfigSummary {
        throw Self.unsupported
    }

    func configChanges(base: String?, draft: String, fileName: String) throws -> ConfigChanges {
        ConfigChanges()
    }

    func catalogReferences(text: String, fileName: String, catalog: String) throws -> [SelectionEntry] {
        []
    }

    func validateCatalogName(text: String, fileName: String, name: String, renaming: String?) throws {}

    func describeSource(_ source: String, gitRef: String?) throws -> SourceInfo {
        let name = URL(fileURLWithPath: source).deletingPathExtension().lastPathComponent
        return SourceInfo(kind: .local(path: source), proposedName: name)
    }

    func supportedAgentTools() -> [AgentToolInfo] {
        [
            AgentToolInfo(id: "claude", displayName: "Claude Code", limitations: []),
            AgentToolInfo(id: "codex", displayName: "Codex", limitations: []),
            AgentToolInfo(id: "cursor", displayName: "Cursor", limitations: []),
            AgentToolInfo(id: "opencode", displayName: "OpenCode", limitations: []),
            AgentToolInfo(id: "vscode", displayName: "VS Code", limitations: []),
        ]
    }

    private static let unsupported = EngineError.internal(
        message: "The fake engine does not edit configs.", detail: [])
}

private struct FakeSetupSession: SetupSessionService {
    let root: String
    let engine: FakeEngineService

    private var fixture: FakeEngineService.Fixture { engine.fixture(root: root) }

    func snapshot() async -> SetupSnapshot {
        SetupSnapshot(root: root, config: fixture.config)
    }

    func loadCatalogs(draftText: String?, policy: FetchPolicy, progress: ProgressHandler?) async throws -> CatalogsState {
        CatalogsState(catalogs: fixture.catalogs)
    }

    func verifyCatalog(name: String, source: String, gitRef: String?, progress: ProgressHandler?) async throws -> CatalogProbe {
        CatalogProbe(name: name, sourceKind: .local(path: source), commit: nil, counts: ItemCounts())
    }

    func browse(draftText: String?) async throws -> BrowseResult {
        BrowseResult(items: fixture.items, problems: [])
    }

    func skillDocument(catalog: String, name: String) async throws -> SkillDocument {
        SkillDocument(frontmatter: [FrontmatterField(key: "name", value: name)], body: "", path: "")
    }

    func packContents(catalog: String, name: String) async throws -> [ItemRef] { [] }

    func ruleMatches(draftText: String?, entry: SelectionEntry) async throws -> [ItemRef] { [] }

    func removalImpact(draftText: String?, item: ItemRef) async throws -> RemovalImpact {
        RemovalImpact(item: item, sustaining: [], effects: [])
    }

    func review(draftText: String?, progress: ProgressHandler?) async throws -> ReviewHandle {
        ReviewHandle(summary: Self.emptySummary)
    }

    func checkCatalogUpdate(catalog: String, progress: ProgressHandler?) async throws -> CatalogUpdateCheck {
        CatalogUpdateCheck(catalog: catalog, freshness: .current, commit: nil, latest: nil, changes: BundleDiff())
    }

    func reviewCatalogUpdates(_ updates: [ReviewedRevision], progress: ProgressHandler?) async throws -> ReviewHandle {
        ReviewHandle(summary: Self.emptySummary)
    }

    func apply(_ review: ReviewHandle, progress: ProgressHandler?) async throws -> ApplyOutcome {
        .installed(summary: InstallSummary(writes: review.summary.writes, removals: review.summary.removals))
    }

    func retryInstall(progress: ProgressHandler?) async throws -> ApplyOutcome {
        .installed(summary: InstallSummary(writes: [], removals: []))
    }

    func status() async throws -> SetupStatus {
        SetupStatus(items: [], artifacts: [])
    }

    func health() async throws -> HealthReport {
        HealthReport(checks: [], findings: [], items: [])
    }

    private static let emptySummary = ReviewSummary(
        config: ConfigChanges(), diff: BundleDiff(), writes: [], removals: [], skipped: [], limitations: [],
        lockChanged: false, blockers: [], canApply: true)
}
