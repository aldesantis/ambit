// The Catalogs area of one setup: the configured catalogs with how each one loaded, the
// selections that no longer resolve, and the add, edit and remove flows.
//
// Reading is offline. Opening the area loads the catalogs from the cache only (`.cacheOnly`); a
// catalog missing from the cache shows Load Catalogs, which fetches what is missing through the
// app's `OperationRunner`. Nothing here checks for newer revisions, which is the separate catalog
// update check.
//
// Every change is staged in the setup's draft through `SetupModel.stage(_:)`; nothing is written.
// Add and source changes are verified first by loading a candidate draft that holds the change
// (fetching it if needed), so the user sees access, path, parsing and revision errors before the
// change is staged.

import Foundation
import Observation

@MainActor
@Observable
final class CatalogsModel {
    /// One configured catalog as the list shows it.
    struct Row: Identifiable, Hashable {
        enum Change: Hashable {
            /// Not in the saved config.
            case added
            /// In the saved config with another source or revision.
            case changed
        }

        var entry: CatalogEntry
        /// `nil` until the catalogs were read.
        var availability: CatalogAvailability?
        var change: Change?

        var id: String { entry.name }
    }

    /// What removing a catalog takes with it, shown before the user confirms.
    struct Removal: Identifiable, Hashable {
        var catalog: CatalogEntry
        /// Entries that select one item of the catalog.
        var selections: [SelectionEntry]
        /// Entries whose pattern selects through a wildcard.
        var rules: [SelectionEntry]

        var id: String { catalog.name }
    }

    unowned let setup: SetupModel

    /// How each catalog of the current config loaded, by name.
    private(set) var states: [String: CatalogAvailability] = [:]
    /// Set when the catalogs could not be read at all, for example because the config is broken.
    private(set) var readError: EngineError?
    private(set) var isReading = false
    /// The current config's entries that match nothing in a loaded catalog. Any blocks Apply.
    private(set) var unmatched: [UnmatchedEntry] = []
    /// True while Load Catalogs fetches.
    private(set) var isFetching = false

    /// The open add or edit sheet.
    var editor: CatalogEditorModel?
    /// The open remove confirmation.
    var removal: Removal?
    /// A failure outside the sheets, shown in an alert.
    var presentedError: PresentedError?

    @ObservationIgnored private var fetchTask: Task<Void, Never>?
    @ObservationIgnored private var readGeneration = 0

    init(setup: SetupModel) {
        self.setup = setup
    }

    var rows: [Row] {
        guard let summary = setup.configSummary else {
            return []
        }

        let saved = setup.savedCatalogs
        return summary.catalogs.map { entry in
            let change: Row.Change? =
                if let old = saved.first(where: { $0.name == entry.name }) {
                    old.source == entry.source && old.gitRef == entry.gitRef ? nil : .changed
                } else if setup.hasPendingChanges {
                    .added
                } else {
                    nil
                }
            return Row(entry: entry, availability: states[entry.name], change: change)
        }
    }

    /// True when some catalog is missing from the cache, so Load Catalogs has work to do.
    var hasMissing: Bool {
        rows.contains { $0.availability == .notCached }
    }

    /// True when Load Catalogs should be offered: something is missing or failed to load.
    var canLoad: Bool {
        rows.contains { row in
            switch row.availability {
            case .notCached, .failed: true
            case .available, nil: false
            }
        }
    }

    // MARK: Reading

    /// Reads the catalogs of the current config from the cache, and which entries no longer
    /// resolve. Never fetches.
    func refresh() async {
        readGeneration += 1
        let generation = readGeneration

        guard setup.configSummary != nil else {
            states = [:]
            unmatched = []
            readError = nil
            return
        }

        isReading = true
        defer {
            if generation == readGeneration {
                isReading = false
            }
        }

        let draftText = setup.draftText
        do {
            let loaded = try await setup.session.loadCatalogs(draftText: draftText, policy: .cacheOnly, progress: nil)
            let unmatched = try await setup.session.unmatchedEntries(draftText: draftText)
            guard generation == readGeneration else {
                return
            }
            take(loaded, unmatched: unmatched)
            readError = nil
        } catch {
            guard generation == readGeneration else {
                return
            }
            readError = EngineError(error)
        }
    }

    /// Fetches the catalogs missing from the cache at their configured revisions, and retries the
    /// ones that failed. Does not look for newer revisions of cached catalogs.
    func loadMissing() {
        guard fetchTask == nil else {
            return
        }

        isFetching = true
        fetchTask = Task {
            defer {
                isFetching = false
                fetchTask = nil
            }

            let session = setup.session
            let draftText = setup.draftText
            do {
                let loaded = try await setup.operations.run(String(localized: "Loading catalogs"), setup: setup.id) {
                    progress in
                    try await session.loadCatalogs(draftText: draftText, policy: .fetchMissing, progress: progress)
                }
                let unmatched = try await session.unmatchedEntries(draftText: draftText)
                readGeneration += 1
                take(loaded, unmatched: unmatched)
                readError = nil
            } catch where error.isCancellation {
                return
            } catch {
                readError = EngineError(error)
            }
        }
    }

    func cancelLoad() {
        fetchTask?.cancel()
    }

    /// Waits for a running Load Catalogs. For tests.
    func waitForLoad() async {
        await fetchTask?.value
    }

    private func take(_ loaded: CatalogsState, unmatched: [UnmatchedEntry]) {
        states = Dictionary(loaded.catalogs.map { ($0.name, $0.availability) }, uniquingKeysWith: { first, _ in first })
        // An entry of a catalog that did not load matches nothing only because nothing was read.
        self.unmatched = unmatched.filter { item in
            guard let catalog = item.entry.catalog else {
                return true
            }
            if case .available = states[catalog] {
                return true
            }
            return false
        }
    }

    // MARK: Editing

    func startAdding() {
        editor = CatalogEditorModel(setup: setup, mode: .add)
    }

    func startEditing(_ name: String) {
        guard let entry = setup.configSummary?.catalogs.first(where: { $0.name == name }) else {
            return
        }
        editor = CatalogEditorModel(setup: setup, mode: .edit(entry))
    }

    /// Stages the editor's change and closes it. On failure the editor stays open with the error.
    func saveEditor() {
        guard let editor else {
            return
        }

        if editor.save() {
            self.editor = nil
        }
    }

    /// Closes the editor without staging anything, canceling a running verification. A
    /// verification may have loaded a candidate config, so the catalogs are read again.
    func cancelEditor() async {
        guard let editor else {
            return
        }

        let verified = editor.didLoadCandidate
        editor.cancelVerification()
        self.editor = nil
        if verified {
            await refresh()
        }
    }

    // MARK: Removing

    /// Shows what removing `name` takes with it. Stages nothing.
    func startRemoving(_ name: String) {
        guard let entry = setup.configSummary?.catalogs.first(where: { $0.name == name }),
            let text = setup.currentConfigText
        else {
            return
        }

        do {
            let references = try setup.engine.catalogReferences(text: text, fileName: setup.fileName, catalog: name)
            removal = Removal(
                catalog: entry, selections: references.filter { !$0.isRule }, rules: references.filter(\.isRule))
        } catch {
            presentedError = PresentedError(title: String(localized: "Could not remove \(name)."), error: error)
        }
    }

    /// Stages removing the catalog together with every entry that references it.
    func confirmRemoval() {
        guard let removal else {
            return
        }

        self.removal = nil
        do {
            try setup.stage([.removeCatalog(name: removal.catalog.name)])
        } catch {
            presentedError = PresentedError(
                title: String(localized: "Could not remove \(removal.catalog.name)."), error: error)
        }
    }

    /// Closes the confirmation. Nothing changes.
    func cancelRemoval() {
        removal = nil
    }

    /// Stages removing one entry that no longer resolves.
    func removeUnmatched(_ entry: SelectionEntry) {
        do {
            try setup.stage([.removeEntry(entry)])
        } catch {
            presentedError = PresentedError(title: String(localized: "Could not remove the selection."), error: error)
        }
    }
}

/// The add and edit catalog sheet. Proposes a name from the source, validates the name with the
/// engine, and verifies a new or changed source by loading it before anything is staged.
@MainActor
@Observable
final class CatalogEditorModel: Identifiable {
    enum Mode: Hashable {
        case add
        case edit(CatalogEntry)
    }

    /// What loading the candidate source found.
    struct Probe: Hashable {
        var commit: String?
        var local: Bool
        /// `nil` when the items could not be listed.
        var counts: ItemCounts?
        /// The catalog's entries that the new source no longer resolves. They block Apply.
        var unmatched: [UnmatchedEntry]
    }

    enum Verification: Hashable {
        case idle
        case verifying
        case verified(Probe)
        case failed(EngineError)
    }

    unowned let setup: SetupModel
    let mode: Mode

    var source: String
    var revision: String
    /// Whether the advanced revision field is shown.
    var showsRevision: Bool
    /// An error from the last save attempt.
    private(set) var saveError: EngineError?
    /// True after a verification loaded a candidate config into the session.
    private(set) var didLoadCandidate = false

    private var customName: String?
    private var verificationState: Verification = .idle
    /// The source and revision the verification state belongs to.
    private var verifiedKey: [String?]?
    @ObservationIgnored private var verifyTask: Task<Void, Never>?

    init(setup: SetupModel, mode: Mode) {
        self.setup = setup
        self.mode = mode
        switch mode {
        case .add:
            source = ""
            revision = ""
            showsRevision = false
        case let .edit(entry):
            source = entry.source
            revision = entry.gitRef ?? ""
            showsRevision = entry.gitRef != nil
            customName = entry.name
        }
    }

    var isAdding: Bool { mode == .add }

    var originalName: String? {
        if case let .edit(entry) = mode { entry.name } else { nil }
    }

    var trimmedSource: String { source.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// The revision to save. `nil` means the default branch, and always for a local folder.
    var gitRef: String? {
        let value = revision.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty || isLocal ? nil : value
    }

    /// What the engine reads the source as. `nil` while the source is empty.
    var sourceInfo: Result<SourceInfo, EngineError>? {
        guard !trimmedSource.isEmpty else {
            return nil
        }
        do {
            return .success(try setup.engine.describeSource(trimmedSource, gitRef: rawRevision))
        } catch {
            return .failure(EngineError(error))
        }
    }

    var sourceKind: SourceKind? {
        if case let .success(info) = sourceInfo { info.kind } else { nil }
    }

    var sourceError: EngineError? {
        if case let .failure(error) = sourceInfo { error } else { nil }
    }

    /// Local folders have no revision, so the revision field is hidden for them.
    var isLocal: Bool {
        if case .local = sourceKind { true } else { false }
    }

    /// Where a local source points, with relative paths resolved against the setup root.
    var resolvedLocalPath: String? {
        isLocal ? setup.resolvedLocalPath(trimmedSource) : nil
    }

    /// The name the user typed, or the one proposed from the source.
    var name: String {
        get {
            if let customName {
                return customName
            }
            if case let .success(info) = sourceInfo {
                return info.proposedName
            }
            return ""
        }
        set { customName = newValue }
    }

    var trimmedName: String { name.trimmingCharacters(in: .whitespaces) }

    /// Why the name cannot be used, from the engine's rules, including uniqueness in the setup.
    var nameError: EngineError? {
        guard let text = setup.currentConfigText else {
            return nil
        }
        do {
            try setup.engine.validateCatalogName(
                text: text, fileName: setup.fileName, name: trimmedName, renaming: originalName)
            return nil
        } catch {
            return EngineError(error)
        }
    }

    /// True when the source or revision differs from the saved one, so it must be verified.
    var sourceChanged: Bool {
        switch mode {
        case .add: true
        case let .edit(entry): trimmedSource != entry.source || gitRef != entry.gitRef
        }
    }

    var isRenaming: Bool {
        if let originalName { trimmedName != originalName } else { false }
    }

    /// The verification of the current source and revision. Editing either resets it.
    var verification: Verification {
        verifiedKey == currentKey ? verificationState : .idle
    }

    var isVerifying: Bool { verification == .verifying }

    var canVerify: Bool {
        sourceChanged && sourceInfo.map { (try? $0.get()) != nil } == true && nameError == nil && !isVerifying
    }

    var canSave: Bool {
        guard sourceError == nil, !trimmedSource.isEmpty, nameError == nil, !isVerifying else {
            return false
        }
        guard sourceChanged else {
            return isRenaming
        }
        if case .verified = verification {
            return true
        }
        return false
    }

    /// The edits saving stages: a rename first, then the new source under the new name.
    var edits: [ConfigEdit] {
        switch mode {
        case .add:
            return [.addCatalog(name: trimmedName, source: trimmedSource, gitRef: gitRef)]
        case let .edit(entry):
            var edits: [ConfigEdit] = []
            if isRenaming {
                edits.append(.renameCatalog(from: entry.name, to: trimmedName))
            }
            if sourceChanged {
                edits.append(.setCatalogSource(name: trimmedName, source: trimmedSource, gitRef: gitRef))
            }
            return edits
        }
    }

    /// Uses a folder from the folder picker as the source.
    func useFolder(_ folder: URL) {
        source = setup.catalogSource(forFolder: folder)
        revision = ""
    }

    /// Loads the source as part of a candidate config, fetching it if it is not cached. Runs
    /// through the operation runner, so it waits for other operations and reports progress.
    func verify() {
        guard canVerify, let base = setup.currentConfigText else {
            return
        }

        let key = currentKey
        let name = trimmedName
        let candidate: String
        do {
            candidate = try setup.engine.editConfig(text: base, fileName: setup.fileName, edits: edits).text
        } catch {
            verifiedKey = key
            verificationState = .failed(EngineError(error))
            return
        }

        verifiedKey = key
        verificationState = .verifying
        let setup = setup
        let session = setup.session
        let checksEntries = !isAdding
        verifyTask = Task {
            let result: Verification
            do {
                let loaded = try await setup.operations.run(String(localized: "Verifying \(name)"), setup: setup.id) {
                    progress in
                    try await session.loadCatalogs(draftText: candidate, policy: .fetchMissing, progress: progress)
                }
                didLoadCandidate = true
                result = await Self.verification(
                    of: name, in: loaded, candidate: candidate, session: session, checksEntries: checksEntries)
            } catch where error.isCancellation {
                result = .idle
            } catch {
                result = .failed(EngineError(error))
            }

            if verifiedKey == key {
                verificationState = result
            }
            verifyTask = nil
        }
    }

    func cancelVerification() {
        verifyTask?.cancel()
    }

    /// Waits for a running verification. For tests.
    func waitForVerification() async {
        await verifyTask?.value
    }

    /// Stages the change. Returns false, keeping `saveError`, when the engine refuses it.
    func save() -> Bool {
        guard canSave else {
            return false
        }
        do {
            try setup.stage(edits)
            saveError = nil
            return true
        } catch {
            saveError = EngineError(error)
            return false
        }
    }

    private var rawRevision: String? {
        let value = revision.trimmingCharacters(in: .whitespacesAndNewlines)
        return value.isEmpty ? nil : value
    }

    private var currentKey: [String?] { [trimmedSource, gitRef] }

    private static func verification(
        of name: String, in loaded: CatalogsState, candidate: String, session: any SetupSessionService,
        checksEntries: Bool
    ) async -> Verification {
        guard let availability = loaded.catalogs.first(where: { $0.name == name })?.availability else {
            return .failed(
                .internal(message: String(localized: "The catalog \(name) was not loaded."), detail: []))
        }

        switch availability {
        case let .failed(error):
            return .failed(error)
        case .notCached:
            return .failed(
                .network(
                    message: String(localized: "The catalog \(name) could not be downloaded."), detail: [],
                    kind: .notCached))
        case let .available(commit, local):
            let unmatched: [UnmatchedEntry] =
                if checksEntries {
                    ((try? await session.unmatchedEntries(draftText: candidate)) ?? [])
                        .filter { $0.entry.catalog == name }
                } else {
                    []
                }
            let counts = (try? await session.browse(draftText: candidate)).map { result in
                result.items.filter { $0.item.catalog == name }.reduce(into: ItemCounts()) { counts, item in
                    switch item.item.kind {
                    case .skill: counts.skills += 1
                    case .pack: counts.packs += 1
                    case .mcp: counts.mcps += 1
                    case .hook: counts.hooks += 1
                    }
                }
            }
            return .verified(Probe(commit: commit, local: local, counts: counts, unmatched: unmatched))
        }
    }
}
