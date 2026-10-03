// An in-memory engine for previews, unit tests of UI state, and UI tests that ask for it
// with `AMBIT_TEST_ENGINE=fake`. Setups come from `fixtures`; a root without a fixture has no
// config. Nothing here writes to disk: apply stores the draft as the fixture's valid config.
// `canonicalPath` reads the real filesystem, because project registration needs real folders, and
// so does the first `snapshot()` of a root without a fixture: it takes an `ambit.yml` found there
// as the root's saved config, so UI tests can seed a setup through `AMBIT_TEST_HOME`.
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
        /// How a config catalog with this source loads, overriding the default (available). A
        /// `.notCached` source loads under `.fetchMissing` and stays cached afterwards.
        var sourceStates: [String: CatalogAvailability] = [:]
        /// Item names a catalog with this source lacks: entries selecting them are unmatched.
        var missingItems: [String: Set<String>] = [:]
        /// How long `loadCatalogs(policy: .fetchMissing)` takes, so tests can cancel it.
        var fetchDelay: Duration = .zero
        /// How many `.fetchMissing` loads ran.
        var fetchCount = 0
        var items: [BrowseItem] = []
        /// Computes `selected`, `routes`, rule matches and removal impact from the draft with
        /// `FakeSelectionResolver`, instead of returning `items` as given.
        var resolvesSelection = false
        /// What `skillDocument` returns per skill. Others get a document with only a name.
        var documents: [ItemRef: SkillDocument] = [:]
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

    /// Gives a root without a fixture the `ambit.yml` saved in it, if any.
    fileprivate func seedFromDisk(root: String) {
        let url = URL(fileURLWithPath: root).appending(path: DraftModel.newFileName)
        guard state.withLock({ $0[root] == nil }), let text = try? String(contentsOf: url, encoding: .utf8),
            let config = try? Self.validConfig(text, root: root)
        else {
            return
        }

        state.withLock { fixtures in
            if fixtures[root] == nil {
                fixtures[root] = Fixture(config: config)
            }
        }
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
            throw EngineError.io(
                message: String(localized: "The folder \(path) does not exist."), detail: [], path: path)
        }
        defer { free(resolved) }
        return String(cString: resolved)
    }

    func gitVersion() async throws -> String {
        "git version 0.0.0 (fake)"
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
        let trimmed = source.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else {
            throw EngineError.config(
                message: String(localized: "The source is empty."), detail: [], path: nil, line: nil)
        }

        var base = trimmed
        while base.hasSuffix("/") {
            base.removeLast()
        }
        var name = String(base.split(whereSeparator: { "/:".contains($0) }).last ?? "")
        if name.hasSuffix(".git") {
            name.removeLast(4)
        }
        if name.isEmpty || name == "." || name == ".." {
            name = "catalog"
        }
        return SourceInfo(kind: FakeConfigText.sourceKind(trimmed, gitRef: gitRef), proposedName: name)
    }

    /// The real tool names with made-up file locations.
    func supportedAgentTools() -> [AgentToolInfo] {
        [("claude", "Claude Code"), ("codex", "Codex"), ("cursor", "Cursor"), ("opencode", "OpenCode"), ("vscode", "VS Code")]
            .map { id, name in
                AgentToolInfo(
                    id: id, displayName: name, skillsDir: ".\(id)/skills", mcpFile: ".\(id)/mcp.json",
                    personalMcpFile: ".\(id)/mcp.json", hooksFile: nil, limitations: [])
            }
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
        engine.seedFromDisk(root: root)
        return SetupSnapshot(root: root, config: fixture.config)
    }

    /// The fixture's `catalogs`, then a state for every other catalog of the config: from
    /// `sourceStates` by source, or available.
    func loadCatalogs(draftText: String?, policy: FetchPolicy, progress: ProgressHandler?) async throws -> CatalogsState {
        let fixture = fixture
        guard let summary = summary(draftText) else {
            return CatalogsState(catalogs: fixture.catalogs)
        }

        if policy == .fetchMissing {
            engine.updateFixture(root: root) { $0.fetchCount += 1 }
            for catalog in summary.catalogs {
                await progress?(ProgressEvent(stage: .fetching, subject: catalog.source, current: 0, total: 0))
            }
            try await pause(fixture.fetchDelay)
        }

        var states = fixture.catalogs
        for catalog in summary.catalogs where !states.contains(where: { $0.name == catalog.name }) {
            let local = catalog.sourceKind.map(Self.isLocal) ?? true
            var availability =
                fixture.sourceStates[catalog.source] ?? .available(commit: local ? nil : Self.commit, local: local)
            if availability == .notCached, policy == .fetchMissing {
                availability = .available(commit: Self.commit, local: false)
                engine.updateFixture(root: root) { $0.sourceStates[catalog.source] = availability }
            }
            states.append(CatalogLoadState(name: catalog.name, availability: availability))
        }
        return CatalogsState(catalogs: states)
    }

    func unmatchedEntries(draftText: String?) async throws -> [UnmatchedEntry] {
        let fixture = fixture
        if fixture.resolvesSelection {
            return resolver(fixture).unmatchedEntries(try entries(draftText))
        }

        guard let summary = summary(draftText) else {
            return []
        }

        return summary.requires.compactMap { entry in
            guard let source = summary.catalogs.first(where: { $0.name == entry.catalog })?.source,
                fixture.missingItems[source]?.contains(entry.pattern) == true
            else {
                return nil
            }
            let error = EngineError.resolution(
                message: String(localized: "\(entry.kind.rawValue) \(entry.pattern) matches nothing in \(source)."),
                detail: [])
            return UnmatchedEntry(entry: entry, error: error)
        }
    }

    private static let commit = "0123456789abcdef0123456789abcdef01234567"

    private static func isLocal(_ kind: SourceKind) -> Bool {
        if case .local = kind { true } else { false }
    }

    /// `draftText`, or the saved config, parsed.
    private func summary(_ draftText: String?) -> ConfigSummary? {
        if let draftText {
            return try? FakeConfigText.parse(draftText, fileName: DraftModel.newFileName)
        }
        if case let .valid(_, _, _, summary) = fixture.config {
            return summary
        }
        return nil
    }

    func verifyCatalog(name: String, source: String, gitRef: String?, progress: ProgressHandler?) async throws -> CatalogProbe {
        CatalogProbe(name: name, sourceKind: .local(path: source), commit: nil, counts: ItemCounts())
    }

    func browse(draftText: String?) async throws -> BrowseResult {
        let fixture = fixture
        guard fixture.resolvesSelection else {
            return BrowseResult(items: fixture.items, problems: [])
        }
        return BrowseResult(items: resolver(fixture).browse(try entries(draftText)), problems: [])
    }

    func skillDocument(catalog: String, name: String) async throws -> SkillDocument {
        if let document = fixture.documents[ItemRef(kind: .skill, catalog: catalog, name: name)] {
            return document
        }
        return SkillDocument(frontmatter: [FrontmatterField(key: "name", value: name)], body: "", path: "")
    }

    func packContents(catalog: String, name: String) async throws -> [ItemRef] {
        let fixture = fixture
        guard fixture.resolvesSelection else {
            return []
        }
        return resolver(fixture).packContents(ItemRef(kind: .pack, catalog: catalog, name: name))
    }

    func ruleMatches(draftText: String?, entry: SelectionEntry) async throws -> [ItemRef] {
        let fixture = fixture
        guard fixture.resolvesSelection else {
            return []
        }
        return resolver(fixture).matches(entry).map(\.item)
    }

    func removalImpact(draftText: String?, item: ItemRef) async throws -> RemovalImpact {
        let fixture = fixture
        guard fixture.resolvesSelection else {
            return RemovalImpact(item: item, sustaining: [], effects: [])
        }
        return resolver(fixture).removalImpact(of: item, entries: try entries(draftText))
    }

    func previewRule(draftText: String?, catalog: String, kind: ItemKind, pattern: String) async throws -> [ItemRef] {
        let fixture = fixture
        if let state = fixture.catalogs.first(where: { $0.name == catalog }), case let .failed(error) = state.availability {
            throw error
        }
        return try resolver(fixture).previewRule(catalog: catalog, kind: kind, pattern: pattern)
    }

    private func resolver(_ fixture: FakeEngineService.Fixture) -> FakeSelectionResolver {
        let loaded: Set<String>? =
            fixture.catalogs.isEmpty
            ? nil
            : Set(fixture.catalogs.filter { if case .available = $0.availability { true } else { false } }.map(\.name))
        return FakeSelectionResolver(items: fixture.items, loadedCatalogs: loaded)
    }

    /// The `requires` entries of `draftText`, or of the saved config when it is `nil`.
    private func entries(_ draftText: String?) throws -> [SelectionEntry] {
        if let draftText {
            return try FakeConfigText.parse(draftText, fileName: DraftModel.newFileName).requires
        }
        if case let .valid(_, _, _, summary) = fixture.config {
            return summary.requires
        }
        return []
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
        SetupStatus(items: [], artifacts: [])
    }

    func health() async throws -> HealthReport {
        HealthReport(checks: [], findings: [], items: [])
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
                    summary.catalogs[summary.catalogs.count - 1].sourceKind = sourceKind(rest, gitRef: nil)
                case "ref" where !summary.catalogs.isEmpty:
                    summary.catalogs[summary.catalogs.count - 1].gitRef = rest
                    summary.catalogs[summary.catalogs.count - 1].sourceKind = sourceKind(
                        summary.catalogs[summary.catalogs.count - 1].source, gitRef: rest)
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
            summary.catalogs.append(
                CatalogEntry(name: name, source: source, gitRef: gitRef, sourceKind: sourceKind(source, gitRef: gitRef)))
        case let .setCatalogSource(name, source, gitRef):
            guard let index = summary.catalogs.firstIndex(where: { $0.name == name }) else {
                throw refuse(String(localized: "There is no catalog named \(name)."))
            }
            summary.catalogs[index].source = source
            summary.catalogs[index].gitRef = gitRef
            summary.catalogs[index].sourceKind = sourceKind(source, gitRef: gitRef)
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

    /// Git for URLs, `git@host:path` remotes and `owner/repo` shorthands (GitHub); local otherwise.
    static func sourceKind(_ source: String, gitRef: String?) -> SourceKind {
        let commitRef = gitRef.map { $0.count == 40 && $0.allSatisfy(\.isHexDigit) } ?? false
        if source.contains("://") {
            return .git(url: source, github: source.contains("github.com"), commitRef: commitRef)
        }
        if source.hasPrefix("git@") {
            return .git(url: source, github: source.hasPrefix("git@github.com:"), commitRef: commitRef)
        }
        let parts = source.split(separator: "/", omittingEmptySubsequences: false)
        if parts.count == 2, !source.hasPrefix("."), !source.hasPrefix("~"), parts.allSatisfy({ !$0.isEmpty }) {
            return .git(url: "https://github.com/\(source).git", github: true, commitRef: commitRef)
        }
        return .local(path: source)
    }

    private static func split(_ value: String) -> (key: String, rest: String) {
        guard let colon = value.firstIndex(of: ":") else {
            return (value, "")
        }
        let rest = value[value.index(after: colon)...].trimmingCharacters(in: .whitespaces)
        return (String(value[..<colon]), rest)
    }
}
