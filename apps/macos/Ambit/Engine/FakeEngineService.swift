// An in-memory engine for previews, unit tests of UI state, and UI tests until the UniFFI-backed
// `LiveEngineService` is wired in. Setups come from `fixtures`; a root without a fixture has no
// config, unless the root holds an `ambit.yml` in the fake's format, which UI tests write to seed a
// setup. Nothing here writes to disk: apply stores the draft as the fixture's valid config.
// `canonicalPath` and that fallback are the only calls that read the real filesystem.
//
// Configs use a small YAML subset of the fake's own (see `FakeConfigText`). Edits re-render the
// whole file, so the fake does not preserve comments; that guarantee belongs to the real engine.

import Foundation
import Synchronization

final class FakeEngineService: EngineService {
    /// Makes `apply` or `retryInstall` fail.
    enum Failure: Sendable, Hashable {
        /// Throws before anything is saved.
        case beforeSave(EngineError)
        /// Saves the config, then reports `notFullyInstalled`.
        case afterSave(EngineError)
    }

    struct Fixture: Sendable {
        var config: ConfigState = .missing
        var catalogs: [CatalogLoadState] = []
        var items: [BrowseItem] = []
        /// Added to every review of this root; any blocker makes the review unappliable.
        var blockers: [Blocker] = []
        var applyFailure: Failure?
        var retryFailure: EngineError?
        /// How long review and apply pause before their first write, so tests can cancel them.
        var delay: Duration = .zero
        /// How many applies saved a config.
        var saveCount = 0
        /// How many applies and retries ran, including failed ones.
        var applyCount = 0
        /// What `status()` reports. `nil` reports no items.
        var status: SetupStatus?
        /// What `health()` reports. `nil` reports every check passing, with `status`'s items.
        var health: HealthReport?
        /// Thrown by `status()` and `health()` instead of a result.
        var statusFailure: EngineError?
        /// How many times `status()` and `health()` ran.
        var statusReads = 0
        var healthChecks = 0
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

    func hasFixture(root: String) -> Bool {
        state.withLock { $0[root] != nil }
    }

    func updateFixture(root: String, _ change: (inout Fixture) -> Void) {
        state.withLock { fixtures in
            var fixture = fixtures[root] ?? Fixture()
            change(&fixture)
            fixtures[root] = fixture
        }
    }

    /// A valid config state for `text` saved at `root`/`fileName`, as `snapshot()` reports it.
    static func validConfig(_ text: String, root: String, fileName: String = "ambit.yml") throws -> ConfigState {
        .valid(
            path: URL(fileURLWithPath: root).appending(path: fileName).path, fileName: fileName, text: text,
            summary: try FakeConfigText.parse(text, fileName: fileName))
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
        FakeConfigText.render(ConfigSummary(harnesses: harnesses, catalogs: [], requires: []))
    }

    func editConfig(text: String, fileName: String, edits: [ConfigEdit]) throws -> EditedConfig {
        var summary = try FakeConfigText.parse(text, fileName: fileName)
        for edit in edits {
            try FakeConfigText.apply(edit, to: &summary, fileName: fileName)
        }
        return EditedConfig(text: FakeConfigText.render(summary), summary: summary)
    }

    func parseConfig(text: String, fileName: String) throws -> ConfigSummary {
        try FakeConfigText.parse(text, fileName: fileName)
    }

    func configChanges(base: String?, draft: String, fileName: String) throws -> ConfigChanges {
        let old = try base.map { try FakeConfigText.parse($0, fileName: fileName) }
            ?? ConfigSummary(harnesses: [], catalogs: [], requires: [])
        let new = try FakeConfigText.parse(draft, fileName: fileName)

        var changes = ConfigChanges()
        changes.harnessesAdded = new.harnesses.filter { !old.harnesses.contains($0) }
        changes.harnessesRemoved = old.harnesses.filter { !new.harnesses.contains($0) }
        for catalog in new.catalogs {
            if let previous = old.catalogs.first(where: { $0.name == catalog.name }) {
                if previous != catalog {
                    changes.catalogsChanged.append(CatalogChange(old: previous, new: catalog))
                }
            } else {
                changes.catalogsAdded.append(catalog)
            }
        }
        changes.catalogsRemoved = old.catalogs.filter { catalog in !new.catalogs.contains { $0.name == catalog.name } }
        changes.entriesAdded = new.requires.filter { !old.requires.contains($0) }
        changes.entriesRemoved = old.requires.filter { !new.requires.contains($0) }
        return changes
    }

    func catalogReferences(text: String, fileName: String, catalog: String) throws -> [SelectionEntry] {
        try FakeConfigText.parse(text, fileName: fileName).requires.filter { $0.catalog == catalog }
    }

    func validateCatalogName(text: String, fileName: String, name: String, renaming: String?) throws {
        let summary = try FakeConfigText.parse(text, fileName: fileName)
        if name.isEmpty || name.contains("/") || (name != renaming && summary.catalogs.contains { $0.name == name }) {
            throw EngineError.config(
                message: String(localized: "\(name) is not a valid catalog name."), detail: [], path: nil, line: nil)
        }
    }

    func describeSource(_ source: String, gitRef: String?) throws -> SourceInfo {
        let name = URL(fileURLWithPath: source).deletingPathExtension().lastPathComponent
        return SourceInfo(kind: .local(path: source), proposedName: name)
    }

    /// The five tools as the engine describes them, with an abridged set of limitations.
    func supportedAgentTools() -> [AgentToolInfo] {
        [
            AgentToolInfo(
                id: "claude", displayName: "Claude Code", skillsDir: ".claude/skills", mcpFile: ".mcp.json",
                personalMcpFile: ".claude.json", hooksFile: ".claude/settings.json", limitations: []),
            AgentToolInfo(
                id: "codex", displayName: "Codex", skillsDir: ".agents/skills", mcpFile: ".codex/config.toml",
                personalMcpFile: ".codex/config.toml", hooksFile: ".codex/hooks.json",
                limitations: ["In a project, hook scripts are found only when Codex runs from the project folder."]),
            AgentToolInfo(
                id: "cursor", displayName: "Cursor", skillsDir: ".claude/skills", mcpFile: ".cursor/mcp.json",
                personalMcpFile: ".cursor/mcp.json", hooksFile: ".cursor/hooks.json",
                limitations: ["In a project, hook scripts are found only when Cursor runs from the project folder."]),
            AgentToolInfo(
                id: "opencode", displayName: "OpenCode", skillsDir: ".agents/skills",
                mcpFile: ".opencode/opencode.jsonc", personalMcpFile: ".opencode/opencode.jsonc", hooksFile: nil,
                limitations: ["Hooks are not supported. Hooks you select are skipped for OpenCode."]),
            AgentToolInfo(
                id: "vscode", displayName: "VS Code", skillsDir: ".agents/skills", mcpFile: ".vscode/mcp.json",
                personalMcpFile: ".vscode/mcp.json", hooksFile: ".claude/settings.json",
                limitations: ["Hooks are written to Claude Code's settings file, which VS Code also reads."]),
        ]
    }
}

/// What a fake review planned, checked again by `apply`.
private struct FakeReviewBacking: Sendable {
    var base: ConfigState
    var text: String
    var fileName: String
}

private struct FakeSetupSession: SetupSessionService {
    let root: String
    let engine: FakeEngineService

    private var fixture: FakeEngineService.Fixture { engine.fixture(root: root) }

    func snapshot() async throws -> SetupSnapshot {
        if !engine.hasFixture(root: root) {
            let file = URL(fileURLWithPath: root).appending(path: DraftModel.newFileName)
            if let text = try? String(contentsOf: file, encoding: .utf8) {
                engine.setFixture(
                    FakeEngineService.Fixture(config: try FakeEngineService.validConfig(text, root: root)), root: root)
            }
        }
        return SetupSnapshot(root: root, config: fixture.config)
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
        let fixture = fixture
        let saved: (text: String, fileName: String)? =
            if case let .valid(_, fileName, text, _) = fixture.config { (text, fileName) } else { nil }
        let fileName = saved?.fileName ?? DraftModel.newFileName
        guard let text = draftText ?? saved?.text else {
            throw EngineError.config(
                message: String(localized: "There is no configuration to review."), detail: [], path: nil, line: nil)
        }

        await progress?(ProgressEvent(stage: .resolving, subject: "", current: 0, total: 0))
        try await pause(fixture.delay)
        await progress?(ProgressEvent(stage: .planning, subject: "", current: 0, total: 0))

        let config = try engine.configChanges(base: saved?.text, draft: text, fileName: fileName)
        var writes: [PlannedWrite] = []
        if text != saved?.text {
            writes.append(PlannedWrite(path: URL(fileURLWithPath: root).appending(path: fileName).path, kind: "config"))
        }
        writes.append(PlannedWrite(path: URL(fileURLWithPath: root).appending(path: "ambit.lock").path, kind: "lock"))

        let summary = ReviewSummary(
            config: config, diff: BundleDiff(), writes: writes, removals: [], skipped: [], limitations: [],
            lockChanged: saved == nil, blockers: fixture.blockers, canApply: fixture.blockers.isEmpty)
        return ReviewHandle(
            summary: summary, backing: FakeReviewBacking(base: fixture.config, text: text, fileName: fileName))
    }

    func checkCatalogUpdate(catalog: String, progress: ProgressHandler?) async throws -> CatalogUpdateCheck {
        CatalogUpdateCheck(catalog: catalog, freshness: .current, commit: nil, latest: nil, changes: BundleDiff())
    }

    func reviewCatalogUpdates(_ updates: [ReviewedRevision], progress: ProgressHandler?) async throws -> ReviewHandle {
        try await review(draftText: nil, progress: progress)
    }

    func apply(_ review: ReviewHandle, progress: ProgressHandler?) async throws -> ApplyOutcome {
        guard let backing = review.backing as? FakeReviewBacking else {
            throw EngineError.internal(message: "The review did not come from the fake engine.", detail: [])
        }

        let fixture = fixture
        engine.updateFixture(root: root) { $0.applyCount += 1 }
        await progress?(ProgressEvent(stage: .checkingOwnership, subject: "", current: 0, total: 0))
        try await pause(fixture.delay)

        guard fixture.config == backing.base else {
            throw EngineError.staleReview(
                message: String(localized: "The setup changed after it was reviewed."), detail: [])
        }
        if case let .beforeSave(error) = fixture.applyFailure {
            throw error
        }

        await progress?(ProgressEvent(stage: .savingConfig, subject: backing.fileName, current: 1, total: 1))
        let config = try FakeEngineService.validConfig(backing.text, root: root, fileName: backing.fileName)
        engine.updateFixture(root: root) { fixture in
            fixture.config = config
            fixture.saveCount += 1
        }

        let writes = review.summary.writes
        for (index, write) in writes.enumerated() {
            await progress?(
                ProgressEvent(
                    stage: .writingFiles, subject: write.path, current: UInt32(index + 1), total: UInt32(writes.count)))
        }

        if case let .afterSave(error) = fixture.applyFailure {
            return .notFullyInstalled(saved: true, error: error)
        }
        return .installed(summary: InstallSummary(writes: writes, removals: review.summary.removals))
    }

    func retryInstall(progress: ProgressHandler?) async throws -> ApplyOutcome {
        let fixture = fixture
        engine.updateFixture(root: root) { $0.applyCount += 1 }
        await progress?(ProgressEvent(stage: .writingFiles, subject: "", current: 0, total: 0))

        if let error = fixture.retryFailure {
            return .notFullyInstalled(saved: true, error: error)
        }
        return .installed(summary: InstallSummary(writes: [], removals: []))
    }

    func status() async throws -> SetupStatus {
        engine.updateFixture(root: root) { $0.statusReads += 1 }
        let fixture = fixture
        if let error = fixture.statusFailure {
            throw error
        }
        return fixture.status ?? SetupStatus(items: [], artifacts: [])
    }

    func health() async throws -> HealthReport {
        engine.updateFixture(root: root) { $0.healthChecks += 1 }
        let fixture = fixture
        if let error = fixture.statusFailure {
            throw error
        }
        if let health = fixture.health {
            return health
        }
        let checks = ["expects", "lock", "ownership", "drift", "mode", "harness"].map {
            HealthCheck(name: $0, passed: true, message: "")
        }
        return HealthReport(checks: checks, findings: [], items: fixture.status?.items ?? [])
    }

    /// Sleeps for `delay`, translating task cancellation into the engine's error.
    private func pause(_ delay: Duration) async throws {
        do {
            if delay > .zero {
                try await Task.sleep(for: delay)
            }
            try Task.checkCancellation()
        } catch {
            throw EngineError.canceled
        }
    }
}

/// The fake's config format: top-level `version`, `harnesses`, `catalogs` and `requires`, with
/// one item per line except catalogs, whose `source` and `ref` follow on indented lines.
///
/// ```yaml
/// version: 1
/// harnesses:
///   - claude
/// catalogs:
///   - name: team
///     source: ./catalog
/// requires:
///   - skill: team/review
/// ```
enum FakeConfigText {
    static func render(_ summary: ConfigSummary) -> String {
        var lines = ["version: 1"]

        lines.append(summary.harnesses.isEmpty ? "harnesses: []" : "harnesses:")
        lines += summary.harnesses.map { "  - \($0)" }

        lines.append(summary.catalogs.isEmpty ? "catalogs: []" : "catalogs:")
        for catalog in summary.catalogs {
            lines.append("  - name: \(catalog.name)")
            lines.append("    source: \(catalog.source)")
            if let ref = catalog.gitRef {
                lines.append("    ref: \(ref)")
            }
        }

        lines.append(summary.requires.isEmpty ? "requires: []" : "requires:")
        for entry in summary.requires {
            let target = entry.catalog.map { "\($0)/\(entry.pattern)" } ?? entry.pattern
            lines.append("  - \(entry.kind.rawValue): \(target)")
        }

        return lines.joined(separator: "\n") + "\n"
    }

    /// Throws `EngineError.config` with the 1-based line of the first line it cannot read.
    static func parse(_ text: String, fileName: String) throws -> ConfigSummary {
        var summary = ConfigSummary(harnesses: [], catalogs: [], requires: [])
        var section = ""

        for (index, raw) in text.split(separator: "\n", omittingEmptySubsequences: false).enumerated() {
            let line = String(raw)
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.isEmpty || trimmed.hasPrefix("#") {
                continue
            }

            func fail() -> EngineError {
                .config(
                    message: String(localized: "\(fileName) line \(index + 1) is not valid."), detail: [line],
                    path: nil, line: UInt32(index + 1))
            }

            if !line.hasPrefix(" ") {
                guard let colon = trimmed.firstIndex(of: ":") else {
                    throw fail()
                }
                section = String(trimmed[..<colon])
                continue
            }

            let value = trimmed.hasPrefix("- ") ? String(trimmed.dropFirst(2)) : trimmed
            let (key, rest) = split(value)
            switch section {
            case "harnesses":
                summary.harnesses.append(value)
            case "catalogs":
                switch key {
                case "name" where trimmed.hasPrefix("- "):
                    summary.catalogs.append(CatalogEntry(name: rest, source: "", gitRef: nil, sourceKind: nil))
                case "source" where !summary.catalogs.isEmpty:
                    summary.catalogs[summary.catalogs.count - 1].source = rest
                    summary.catalogs[summary.catalogs.count - 1].sourceKind = .local(path: rest)
                case "ref" where !summary.catalogs.isEmpty:
                    summary.catalogs[summary.catalogs.count - 1].gitRef = rest
                default:
                    throw fail()
                }
            case "requires":
                guard let kind = ItemKind(rawValue: key), !rest.isEmpty else {
                    throw fail()
                }
                let parts = rest.split(separator: "/", maxSplits: 1).map(String.init)
                let catalog = parts.count == 2 ? parts[0] : nil
                let pattern = parts.last ?? rest
                summary.requires.append(
                    SelectionEntry(kind: kind, catalog: catalog, pattern: pattern, isRule: pattern.contains("*")))
            default:
                throw fail()
            }
        }

        return summary
    }

    static func apply(_ edit: ConfigEdit, to summary: inout ConfigSummary, fileName: String) throws {
        func refuse(_ message: String) -> EngineError {
            .config(message: message, detail: [], path: nil, line: nil)
        }

        switch edit {
        case let .setHarnesses(harnesses):
            summary.harnesses = harnesses
        case let .addCatalog(name, source, gitRef):
            guard !summary.catalogs.contains(where: { $0.name == name }) else {
                throw refuse(String(localized: "A catalog named \(name) already exists."))
            }
            summary.catalogs.append(CatalogEntry(name: name, source: source, gitRef: gitRef, sourceKind: .local(path: source)))
        case let .setCatalogSource(name, source, gitRef):
            guard let index = summary.catalogs.firstIndex(where: { $0.name == name }) else {
                throw refuse(String(localized: "There is no catalog named \(name)."))
            }
            summary.catalogs[index].source = source
            summary.catalogs[index].gitRef = gitRef
            summary.catalogs[index].sourceKind = .local(path: source)
        case let .renameCatalog(from, to):
            guard let index = summary.catalogs.firstIndex(where: { $0.name == from }) else {
                throw refuse(String(localized: "There is no catalog named \(from)."))
            }
            summary.catalogs[index].name = to
            for entry in summary.requires.indices where summary.requires[entry].catalog == from {
                summary.requires[entry].catalog = to
            }
        case let .removeCatalog(name):
            summary.catalogs.removeAll { $0.name == name }
            summary.requires.removeAll { $0.catalog == name }
        case let .addEntry(entry):
            if !summary.requires.contains(entry) {
                summary.requires.append(entry)
            }
        case let .removeEntry(entry):
            summary.requires.removeAll { $0 == entry }
        case let .replaceEntry(old, new):
            guard let index = summary.requires.firstIndex(of: old) else {
                throw refuse(String(localized: "The entry to replace is not in \(fileName)."))
            }
            summary.requires[index] = new
        }
    }

    private static func split(_ value: String) -> (key: String, rest: String) {
        guard let colon = value.firstIndex(of: ":") else {
            return (value, "")
        }
        let rest = value[value.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        return (String(value[..<colon]), rest)
    }
}
