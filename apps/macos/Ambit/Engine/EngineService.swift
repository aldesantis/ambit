// The only engine API that stores see. It mirrors the UniFFI surface of `crates/ambit-ffi`
// (`Engine`, `SetupSession` and the free edit functions) one method per export, with the Swift
// value types in EngineRecords.swift.
//
// Threading: FFI exports block, so every method that can touch the filesystem, git or the
// network is `async` and the live implementation runs it on a background queue. The pure config
// functions only parse text and stay synchronous. Cancelling the calling Swift task cancels the
// operation (the live implementation forwards it to a `CancelToken`). Mutations are serialized
// by the caller, not here.

import Foundation

protocol EngineService: Sendable {
    /// Replaces the GitHub token used for later git operations. `nil` signs out.
    func setGitHubToken(_ token: String?) async

    /// Opens a setup rooted at `root` without any I/O.
    func openSetup(root: String) -> any SetupSessionService

    /// Resolves symlinks and relative components of an existing path.
    ///
    /// Throws `EngineError.config` when the path does not exist.
    func canonicalPath(_ path: String) async throws -> String

    /// A new config with `harnesses` and empty `catalogs` and `requires`.
    func newConfigText(harnesses: [String]) -> String

    /// Applies `edits` to `text`, preserving comments and formatting outside the edited nodes.
    func editConfig(text: String, fileName: String, edits: [ConfigEdit]) throws -> EditedConfig

    func parseConfig(text: String, fileName: String) throws -> ConfigSummary

    /// The changes from `base` (`nil` for a setup without a config) to `draft`.
    func configChanges(base: String?, draft: String, fileName: String) throws -> ConfigChanges

    /// The `requires` entries that name `catalog`.
    func catalogReferences(text: String, fileName: String, catalog: String) throws -> [SelectionEntry]

    /// Throws `EngineError.config` when `name` is empty, contains `/`, or is taken.
    func validateCatalogName(text: String, fileName: String, name: String, renaming: String?) throws

    func describeSource(_ source: String, gitRef: String?) throws -> SourceInfo

    func supportedAgentTools() -> [AgentToolInfo]
}

/// One setup root. Holds the catalogs loaded by `loadCatalogs` for later browse calls.
protocol SetupSessionService: Sendable {
    var root: String { get }

    /// Reads the config only. Never fetches.
    func snapshot() async -> SetupSnapshot

    /// Loads the catalogs named by `draftText`, or by the saved config when it is `nil`.
    func loadCatalogs(draftText: String?, policy: FetchPolicy, progress: ProgressHandler?) async throws -> CatalogsState

    /// Fetches and reads a catalog that is not part of the config yet.
    func verifyCatalog(name: String, source: String, gitRef: String?, progress: ProgressHandler?) async throws -> CatalogProbe

    /// Every item of the loaded catalogs, resolved against `draftText` or the saved config.
    func browse(draftText: String?) async throws -> BrowseResult

    func skillDocument(catalog: String, name: String) async throws -> SkillDocument

    func packContents(catalog: String, name: String) async throws -> [ItemRef]

    func ruleMatches(draftText: String?, entry: SelectionEntry) async throws -> [ItemRef]

    func removalImpact(draftText: String?, item: ItemRef) async throws -> RemovalImpact

    /// Plans `draftText`, or the saved config when it is `nil`. Never writes.
    func review(draftText: String?, progress: ProgressHandler?) async throws -> ReviewHandle

    func checkCatalogUpdate(catalog: String, progress: ProgressHandler?) async throws -> CatalogUpdateCheck

    func reviewCatalogUpdates(_ updates: [ReviewedRevision], progress: ProgressHandler?) async throws -> ReviewHandle

    /// Saves and installs exactly what `review` planned.
    ///
    /// Throws `EngineError.staleReview` when the setup changed since the review.
    func apply(_ review: ReviewHandle, progress: ProgressHandler?) async throws -> ApplyOutcome

    /// Installs the saved config again after a `notFullyInstalled` outcome.
    func retryInstall(progress: ProgressHandler?) async throws -> ApplyOutcome

    /// Offline.
    func status() async throws -> SetupStatus

    /// Offline.
    func health() async throws -> HealthReport
}
