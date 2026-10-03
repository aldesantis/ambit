// Value types the engine facade exchanges with stores. Each mirrors a UniFFI record or enum in
// `crates/ambit-ffi/src/records.rs` field for field, so `LiveEngineService` converts them with
// plain member-wise mapping. Paths are strings and line numbers are `UInt32`, as in the FFI.

import Foundation

enum Stage: String, Sendable, Hashable, CaseIterable {
    case loadingCatalogs, fetching, resolving, planning, checkingOwnership
    case savingConfig, writingFiles, removingFiles, writingRecords
}

struct ProgressEvent: Sendable, Hashable {
    var stage: Stage
    var subject: String
    var current: UInt32
    var total: UInt32
}

/// Receives progress on the main actor. Swift task cancellation is the cancel signal.
typealias ProgressHandler = @MainActor @Sendable (ProgressEvent) -> Void

enum ItemKind: String, Sendable, Hashable, CaseIterable {
    case skill, pack, mcp, hook
}

enum SourceKind: Sendable, Hashable {
    case local(path: String)
    /// `commitRef` is true when the ref is a full commit id, which never moves.
    case git(url: String, github: Bool, commitRef: Bool)
}

struct CatalogEntry: Sendable, Hashable {
    var name: String
    var source: String
    var gitRef: String?
    /// `nil` when the source string cannot be parsed.
    var sourceKind: SourceKind?
}

/// One `requires` entry. `catalog` is `nil` for an unqualified entry. `isRule` is true when the
/// pattern contains a wildcard.
struct SelectionEntry: Sendable, Hashable {
    var kind: ItemKind
    var catalog: String?
    var pattern: String
    var isRule: Bool
}

struct ConfigSummary: Sendable, Hashable {
    var harnesses: [String]
    var catalogs: [CatalogEntry]
    var requires: [SelectionEntry]
}

/// Why a config cannot be used. `line` locates the problem in the file when known.
struct ConfigProblem: Sendable, Hashable {
    var message: String
    var detail: [String]
    var line: UInt32?
}

enum ConfigState: Sendable, Hashable {
    case missing
    /// More than one config file exists.
    case ambiguous(files: [String], problem: ConfigProblem)
    case invalid(path: String, fileName: String, problem: ConfigProblem)
    case valid(path: String, fileName: String, text: String, summary: ConfigSummary)
}

struct SetupSnapshot: Sendable, Hashable {
    var root: String
    var config: ConfigState
}

enum ConfigEdit: Sendable, Hashable {
    case setHarnesses(harnesses: [String])
    case addCatalog(name: String, source: String, gitRef: String?)
    /// Keeps the catalog's selections.
    case setCatalogSource(name: String, source: String, gitRef: String?)
    /// Rewrites the qualified `requires` entries that name the catalog.
    case renameCatalog(from: String, to: String)
    /// Also removes the catalog's `requires` entries.
    case removeCatalog(name: String)
    case addEntry(SelectionEntry)
    case removeEntry(SelectionEntry)
    case replaceEntry(old: SelectionEntry, new: SelectionEntry)
}

struct EditedConfig: Sendable, Hashable {
    var text: String
    var summary: ConfigSummary
}

struct CatalogChange: Sendable, Hashable {
    var old: CatalogEntry
    var new: CatalogEntry
}

struct CatalogRename: Sendable, Hashable {
    var from: String
    var to: String
}

struct ConfigChanges: Sendable, Hashable {
    var harnessesAdded: [String] = []
    var harnessesRemoved: [String] = []
    var catalogsAdded: [CatalogEntry] = []
    var catalogsRemoved: [CatalogEntry] = []
    var catalogsChanged: [CatalogChange] = []
    var catalogsRenamed: [CatalogRename] = []
    var entriesAdded: [SelectionEntry] = []
    var entriesRemoved: [SelectionEntry] = []
}

struct SourceInfo: Sendable, Hashable {
    var kind: SourceKind
    var proposedName: String
}

struct AgentToolInfo: Sendable, Hashable, Identifiable {
    /// The harness name written to `harnesses`.
    var id: String
    var displayName: String
    /// Paths relative to the setup root.
    var skillsDir: String
    var mcpFile: String
    /// The MCP file of the Personal setup, when it differs from `mcpFile`.
    var personalMcpFile: String
    var hooksFile: String?
    var limitations: [String]
}

enum FetchPolicy: Sendable, Hashable {
    /// Offline: a catalog missing from the cache is reported as `notCached`.
    case cacheOnly
    /// Fetches catalogs missing from the cache, without checking cached ones for newer revisions.
    case fetchMissing
}

enum CatalogAvailability: Sendable, Hashable {
    case available(commit: String?, local: Bool)
    case notCached
    case failed(error: EngineError)
}

struct CatalogLoadState: Sendable, Hashable {
    var name: String
    var availability: CatalogAvailability
}

struct CatalogsState: Sendable, Hashable {
    var catalogs: [CatalogLoadState]
}

struct ItemCounts: Sendable, Hashable {
    var skills: UInt32 = 0
    var packs: UInt32 = 0
    var mcps: UInt32 = 0
    var hooks: UInt32 = 0
}

struct CatalogProbe: Sendable, Hashable {
    var name: String
    var sourceKind: SourceKind
    var commit: String?
    var counts: ItemCounts
}

struct ItemRef: Sendable, Hashable {
    var kind: ItemKind
    var catalog: String
    var name: String
}

/// Why an item is selected. `chain` walks from the requiring item up to a project entry.
enum Route: Sendable, Hashable {
    case direct(entry: SelectionEntry)
    case rule(entry: SelectionEntry)
    case pack(pack: ItemRef, chain: [ItemRef])
    case dependency(requirer: ItemRef, chain: [ItemRef])
}

enum McpTransport: Sendable, Hashable {
    /// Environment variable names only. Values are never exposed.
    case stdio(command: String, args: [String], envNames: [String])
    /// Header names only. Values are never exposed.
    case http(url: String, headerNames: [String])
}

enum ItemDetail: Sendable, Hashable {
    case skill(path: String)
    case pack(requires: [SelectionEntry])
    case mcp(transport: McpTransport)
    case hook(event: String, matcher: String?, hookType: String, command: String?, timeout: UInt32?)
}

struct BrowseItem: Sendable, Hashable {
    var item: ItemRef
    var description: String?
    var requires: [SelectionEntry]
    var expects: [String]
    var selected: Bool
    var routes: [Route]
    var detail: ItemDetail
    /// Configured agent tools that cannot install the item. Only hooks have any.
    var limitations: [ToolLimitation] = []
}

/// An agent tool that skips an item. `message` is the reason in a sentence.
struct ToolLimitation: Sendable, Hashable {
    var tool: String
    var message: String
}

struct BrowseResult: Sendable, Hashable {
    var items: [BrowseItem]
    var problems: [EngineError]
}

struct FrontmatterField: Sendable, Hashable {
    var key: String
    var value: String
}

struct SkillDocument: Sendable, Hashable {
    /// In file order.
    var frontmatter: [FrontmatterField]
    var body: String
    var path: String
}

struct BundleDiff: Sendable, Hashable {
    var added: [ItemRef] = []
    var removed: [ItemRef] = []
}

struct SustainingEntry: Sendable, Hashable {
    var entry: SelectionEntry
    var routes: [Route]
}

struct EntryRemovalEffect: Sendable, Hashable {
    var entry: SelectionEntry
    var diff: BundleDiff
}

/// What uninstalling `item` takes: every entry in `sustaining` must be removed or edited.
struct RemovalImpact: Sendable, Hashable {
    var item: ItemRef
    var sustaining: [SustainingEntry]
    var effects: [EntryRemovalEffect]
    /// What removing every sustaining entry would uninstall: the item and whatever only they kept.
    var removed: [ItemRef] = []
}

/// A `requires` entry that matches nothing, with the error resolution would report.
struct UnmatchedEntry: Sendable, Hashable {
    var entry: SelectionEntry
    var error: EngineError
}

struct PlannedWrite: Sendable, Hashable {
    var path: String
    var kind: String
}

struct PlannedRemoval: Sendable, Hashable {
    var path: String
    var kind: String
}

struct SkippedHook: Sendable, Hashable {
    var hook: ItemRef
    var harness: String
    var reason: String
}

enum Severity: String, Sendable, Hashable {
    case info, warning, error
}

struct Finding: Sendable, Hashable {
    var check: String
    var severity: Severity
    var message: String
    var detail: [String]
    var subject: ItemRef?
    var harness: String?
}

struct OwnershipConflict: Sendable, Hashable {
    var path: String
    var key: String?
    var message: String
    var detail: [String]
}

enum Blocker: Sendable, Hashable {
    case config(error: EngineError)
    case unmatched(entry: SelectionEntry, error: EngineError)
    case resolution(error: EngineError)
    case ownership(conflict: OwnershipConflict)
}

struct ReviewSummary: Sendable, Hashable {
    var config: ConfigChanges
    var diff: BundleDiff
    var writes: [PlannedWrite]
    var removals: [PlannedRemoval]
    var skipped: [SkippedHook]
    var limitations: [Finding]
    var lockChanged: Bool
    var blockers: [Blocker]
    var canApply: Bool
}

/// An opaque reviewed plan. Only the engine that produced it can apply it.
final class ReviewHandle: Sendable {
    let summary: ReviewSummary
    /// The engine's own review object, such as the FFI `Review`.
    let backing: (any Sendable)?

    init(summary: ReviewSummary, backing: (any Sendable)? = nil) {
        self.summary = summary
        self.backing = backing
    }
}

struct InstallSummary: Sendable, Hashable {
    var writes: [PlannedWrite]
    var removals: [PlannedRemoval]
}

enum ApplyOutcome: Sendable, Hashable {
    case installed(summary: InstallSummary)
    /// `saved` is true when the config was written before installation failed.
    case notFullyInstalled(saved: Bool, error: EngineError)
}

enum CatalogFreshness: String, Sendable, Hashable {
    case outdated, current, pinned, local
}

struct CatalogUpdateCheck: Sendable, Hashable {
    var catalog: String
    var freshness: CatalogFreshness
    var commit: String?
    var latest: String?
    var changes: BundleDiff
}

struct ReviewedRevision: Sendable, Hashable {
    var catalog: String
    var commit: String
}

/// Ordered by severity, so the worst of several states is their maximum.
enum ArtifactState: Int, Sendable, Hashable, Comparable {
    case ok, missing, modified, unowned

    static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }
}

struct StatusArtifact: Sendable, Hashable {
    var path: String
    var kind: String
    var state: ArtifactState
    var detail: String?
}

struct ItemStatus: Sendable, Hashable {
    var item: ItemRef
    var state: ArtifactState
    var artifacts: [StatusArtifact]
}

struct SetupStatus: Sendable, Hashable {
    var items: [ItemStatus]
    var artifacts: [StatusArtifact]
}

struct HealthCheck: Sendable, Hashable {
    var name: String
    var passed: Bool
    var message: String
}

struct HealthReport: Sendable, Hashable {
    var checks: [HealthCheck]
    var findings: [Finding]
    var items: [ItemStatus]
}
