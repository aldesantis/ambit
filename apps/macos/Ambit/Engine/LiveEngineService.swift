// The engine facade backed by the Rust engine through UniFFI (the `AmbitEngine` package).
//
// Every FFI export blocks, so each call that touches the filesystem, git or the network runs on
// `LiveEngineExecutor`: reads concurrently, mutations (anything that fetches or writes) one at a
// time. A review handle's `backing` is the FFI `Review`, which `apply` hands back to the engine.

import AmbitEngine
import Foundation

final class LiveEngineService: EngineService {
    private let engine: AmbitEngine.Engine
    private let executor = LiveEngineExecutor()

    /// - Parameter environment: The complete environment, from `LiveEngineEnvironment.make`.
    init(environment: [String: String]) {
        engine = AmbitEngine.Engine(config: AmbitEngine.EngineConfig(env: environment))
    }

    func setGitHubToken(_ token: String?) async {
        let engine = engine
        // Serialized with mutations, so an operation already running keeps the token it started with.
        _ = try? await executor.run(.mutation) { engine.setGithubToken(token: token) }
    }

    func openSetup(root: String) -> any SetupSessionService {
        LiveSetupSession(engine: engine, root: root, executor: executor)
    }

    func canonicalPath(_ path: String) async throws -> String {
        let engine = engine
        return try await executor.run(.read) { try engine.canonicalPath(path: path) }
    }

    func gitVersion() async throws -> String {
        let engine = engine
        return try await executor.run(.read) { try engine.gitVersion() }
    }

    // The config and source functions parse text only, so they run on the caller's thread.

    func newConfigText(harnesses: [String]) -> String {
        AmbitEngine.newConfigText(harnesses: harnesses)
    }

    func editConfig(text: String, fileName: String, edits: [ConfigEdit]) throws -> EditedConfig {
        try LiveEngineMapping.translatingErrors {
            LiveEngineMapping.editedConfig(
                try AmbitEngine.editConfig(
                    text: text, fileName: fileName, edits: edits.map(LiveEngineMapping.configEdit)))
        }
    }

    func parseConfig(text: String, fileName: String) throws -> ConfigSummary {
        try LiveEngineMapping.translatingErrors {
            LiveEngineMapping.configSummary(try AmbitEngine.parseConfig(text: text, fileName: fileName))
        }
    }

    func configChanges(base: String?, draft: String, fileName: String) throws -> ConfigChanges {
        try LiveEngineMapping.translatingErrors {
            LiveEngineMapping.configChanges(
                try AmbitEngine.configChanges(base: base, draft: draft, fileName: fileName))
        }
    }

    func catalogReferences(text: String, fileName: String, catalog: String) throws -> [SelectionEntry] {
        try LiveEngineMapping.translatingErrors {
            try AmbitEngine.catalogReferences(text: text, fileName: fileName, catalog: catalog)
                .map(LiveEngineMapping.selectionEntry)
        }
    }

    func validateCatalogName(text: String, fileName: String, name: String, renaming: String?) throws {
        try LiveEngineMapping.translatingErrors {
            try AmbitEngine.validateCatalogName(text: text, fileName: fileName, name: name, renaming: renaming)
        }
    }

    func describeSource(_ source: String, gitRef: String?) throws -> SourceInfo {
        try LiveEngineMapping.translatingErrors {
            LiveEngineMapping.sourceInfo(try AmbitEngine.describeSource(source: source, gitRef: gitRef))
        }
    }

    func supportedAgentTools() -> [AgentToolInfo] {
        AmbitEngine.supportedAgentTools().map(LiveEngineMapping.agentTool)
    }
}

final class LiveSetupSession: SetupSessionService {
    private let engine: AmbitEngine.Engine
    private let session: AmbitEngine.SetupSession
    private let executor: LiveEngineExecutor
    let root: String

    init(engine: AmbitEngine.Engine, root: String, executor: LiveEngineExecutor) {
        self.engine = engine
        session = engine.openSetup(root: root)
        self.executor = executor
        self.root = session.root()
    }

    func snapshot() async throws -> SetupSnapshot {
        let session = session
        return try await executor.run(.read) { LiveEngineMapping.snapshot(try session.snapshot()) }
    }

    // Long operations go through `executor.operation`, which hands the export a `CancelToken`
    // tied to task cancellation and a `ProgressListener` that reports on the main actor.

    func loadCatalogs(draftText: String?, policy: FetchPolicy, progress: ProgressHandler?) async throws -> CatalogsState {
        let session = session
        let policy = LiveEngineMapping.fetchPolicy(policy)
        // A mutation: it may fetch, and it replaces the session's loaded catalogs.
        return try await executor.operation(.mutation, progress: progress) { cancel, listener in
            LiveEngineMapping.catalogsState(
                try session.loadCatalogs(draftText: draftText, policy: policy, cancel: cancel, listener: listener))
        }
    }

    func verifyCatalog(name: String, source: String, gitRef: String?, progress: ProgressHandler?) async throws
        -> CatalogProbe
    {
        // No export of its own: a throwaway session on the same root (so a relative `path:` source
        // resolves as it will in the config) loads a config naming only this catalog. This
        // session's loaded catalogs stay as they were.
        let probe = engine.openSetup(root: root)
        return try await executor.operation(.mutation, progress: progress) { cancel, listener in
            let sourceKind = LiveEngineMapping.sourceKind(
                try AmbitEngine.describeSource(source: source, gitRef: gitRef).kind)
            let draft = try AmbitEngine.editConfig(
                text: AmbitEngine.newConfigText(harnesses: ["claude"]), fileName: "ambit.yml",
                edits: [.addCatalog(name: name, source: source, gitRef: gitRef)]
            ).text
            let state = try probe.loadCatalogs(
                draftText: draft, policy: .fetchMissing, cancel: cancel, listener: listener)

            var commit: String?
            switch state.catalogs.first?.state {
            case let .loaded(loadedCommit, _):
                commit = loadedCommit
            case let .notCached(error), let .failed(error):
                throw error
            case nil:
                throw EngineError.internal(message: "The engine did not report the catalog.", detail: [])
            }

            var counts = ItemCounts()
            for item in try probe.browse(draftText: draft).items {
                switch item.kind {
                case .skill: counts.skills += 1
                case .pack: counts.packs += 1
                case .mcp: counts.mcps += 1
                case .hook: counts.hooks += 1
                }
            }
            return CatalogProbe(name: name, sourceKind: sourceKind, commit: commit, counts: counts)
        }
    }

    func browse(draftText: String?) async throws -> BrowseResult {
        let session = session
        return try await executor.run(.read) {
            LiveEngineMapping.browseResult(try session.browse(draftText: draftText))
        }
    }

    func skillDocument(catalog: String, name: String) async throws -> SkillDocument {
        let session = session
        return try await executor.run(.read) {
            LiveEngineMapping.skillDocument(try session.skillDocument(catalog: catalog, name: name))
        }
    }

    func packContents(catalog: String, name: String) async throws -> [ItemRef] {
        let session = session
        return try await executor.run(.read) {
            try session.packContents(catalog: catalog, name: name).map(LiveEngineMapping.itemRef)
        }
    }

    /// Throws `EngineError.config` for an entry without a catalog: a rule always names one.
    func ruleMatches(draftText: String?, entry: SelectionEntry) async throws -> [ItemRef] {
        guard let catalog = entry.catalog else {
            throw EngineError.config(
                message: String(localized: "A rule must name its catalog."), detail: [], path: nil, line: nil)
        }
        return try await previewRule(draftText: draftText, catalog: catalog, kind: entry.kind, pattern: entry.pattern)
    }

    func previewRule(draftText: String?, catalog: String, kind: ItemKind, pattern: String) async throws -> [ItemRef] {
        let session = session
        let kind = LiveEngineMapping.itemKind(kind)
        return try await executor.run(.read) {
            try session.previewRule(draftText: draftText, catalog: catalog, kind: kind, pattern: pattern)
                .map(LiveEngineMapping.itemRef)
        }
    }

    func unmatchedEntries(draftText: String?) async throws -> [UnmatchedEntry] {
        let session = session
        return try await executor.run(.read) {
            try session.unmatchedEntries(draftText: draftText).map {
                UnmatchedEntry(entry: LiveEngineMapping.selectionEntry($0.entry), error: LiveEngineMapping.engineError($0.error))
            }
        }
    }

    func removalImpact(draftText: String?, item: ItemRef) async throws -> RemovalImpact {
        let session = session
        let item = LiveEngineMapping.itemRef(item)
        return try await executor.run(.read) {
            LiveEngineMapping.removalImpact(try session.removalImpact(draftText: draftText, item: item))
        }
    }

    /// The config file a review saves to: the existing one, or `ambit.yml` for a new setup.
    private static func configFileName(_ session: AmbitEngine.SetupSession) throws -> String {
        switch try session.snapshot().config {
        case let .valid(_, fileName, _, _), let .invalid(_, fileName, _): fileName
        case let .ambiguous(files, _): files.first ?? "ambit.yml"
        case .missing: "ambit.yml"
        }
    }

    func review(draftText: String?, progress: ProgressHandler?) async throws -> ReviewHandle {
        let session = session
        let review = try await executor.operation(.mutation, progress: progress) { cancel, listener in
            try session.review(
                draftText: draftText, fileName: try Self.configFileName(session), cancel: cancel, progress: listener)
        }
        return ReviewHandle(summary: LiveEngineMapping.reviewSummary(review.summary()), backing: review)
    }

    /// Fetches the catalog. The export takes no cancel token, so cancelling waits for the fetch.
    func checkCatalogUpdate(catalog: String, progress: ProgressHandler?) async throws -> CatalogUpdateCheck {
        let session = session
        return try await executor.run(.mutation) {
            LiveEngineMapping.catalogUpdateCheck(try session.checkCatalogUpdate(catalog: catalog))
        }
    }

    func reviewCatalogUpdates(_ updates: [ReviewedRevision], progress: ProgressHandler?) async throws -> ReviewHandle {
        let session = session
        let updates = updates.map(LiveEngineMapping.reviewedRevision)
        let review = try await executor.operation(.mutation, progress: progress) { cancel, listener in
            try session.reviewCatalogUpdates(updates: updates, cancel: cancel, progress: listener)
        }
        return ReviewHandle(summary: LiveEngineMapping.reviewSummary(review.summary()), backing: review)
    }

    func apply(_ review: ReviewHandle, progress: ProgressHandler?) async throws -> ApplyOutcome {
        guard let backing = review.backing as? AmbitEngine.Review else {
            throw EngineError.internal(message: "This review was not made by the engine applying it.", detail: [])
        }
        let session = session
        let outcome = try await executor.operation(.mutation, progress: progress) { cancel, listener in
            try session.apply(review: backing, cancel: cancel, progress: listener)
        }
        return LiveEngineMapping.applyOutcome(outcome, reviewed: review.summary)
    }

    func retryInstall(progress: ProgressHandler?) async throws -> ApplyOutcome {
        let session = session
        let outcome = try await executor.operation(.mutation, progress: progress) { cancel, listener in
            try session.retryInstall(cancel: cancel, progress: listener)
        }
        return LiveEngineMapping.applyOutcome(outcome, reviewed: nil)
    }

    func status() async throws -> SetupStatus {
        let session = session
        return try await executor.run(.read) { LiveEngineMapping.setupStatus(try session.status()) }
    }

    func health() async throws -> HealthReport {
        let session = session
        return try await executor.run(.read) { LiveEngineMapping.healthReport(try session.health()) }
    }
}
