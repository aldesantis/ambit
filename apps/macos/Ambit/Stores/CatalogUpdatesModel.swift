// Manual catalog updates for one setup: check remote catalogs for newer revisions, then review
// and apply exactly the revisions the check found.
//
// Rules:
// - Checks run only when the user asks. Nothing here runs on launch, on a timer or in the
//   background; results from earlier checks are read back from the app state, never refreshed.
// - Each catalog is checked with its own engine call, so one unreachable catalog does not hide
//   the others' results. A failed check is never shown as up to date.
// - Local catalogs and catalogs pinned to a commit are never checked: their source says
//   everything there is to say.
// - Updates work on the saved config, so they require the draft to be applied or discarded
//   first. The review pins each catalog to the commit the check reported, and apply installs that
//   reviewed plan even if the remote branch moves afterwards.

import Foundation
import Observation

/// The persisted result of one catalog's last check.
struct CatalogCheckRecord: Codable, Hashable, Sendable {
    enum Outcome: String, Codable, Sendable {
        case upToDate, updateAvailable, pinned, localFiles, failed
    }

    var outcome: Outcome
    var checkedAt: Date
    /// The source and ref the check ran against. A record for another source no longer applies.
    var source: String
    var gitRef: String?
    /// The installed commit, when known. Kept from the previous record when a check fails.
    var installed: String?
    var latest: String?
    var added: [CheckedItem] = []
    var removed: [CheckedItem] = []
    var failure: String?

    func applies(to catalog: CatalogEntry) -> Bool {
        source == catalog.source && gitRef == catalog.gitRef
    }
}

struct CheckedItem: Codable, Hashable, Sendable {
    var kind: String
    var catalog: String
    var name: String

    init(_ item: ItemRef) {
        kind = item.kind.rawValue
        catalog = item.catalog
        name = item.name
    }
}

/// One setup's catalog checks, stored in `AppState.catalogChecks` under the setup root path.
struct SetupCatalogChecks: Codable, Equatable, Sendable {
    var lastCheckedAt: Date?
    /// By catalog name.
    var results: [String: CatalogCheckRecord] = [:]

    init(lastCheckedAt: Date? = nil, results: [String: CatalogCheckRecord] = [:]) {
        self.lastCheckedAt = lastCheckedAt
        self.results = results
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        lastCheckedAt = try? container.decodeIfPresent(Date.self, forKey: .lastCheckedAt)
        results = (try? container.decodeIfPresent([String: CatalogCheckRecord].self, forKey: .results)) ?? [:]
    }
}

@MainActor
@Observable
final class CatalogUpdatesModel {
    enum Status: Equatable {
        case unchecked
        case checking
        case upToDate(checkedAt: Date)
        case updateAvailable(CatalogCheckRecord)
        case pinned(commit: String?)
        case localFiles
        /// `lastKnown` is the installed commit from an earlier check, when there was one.
        case failed(message: String, lastKnown: String?, checkedAt: Date)
    }

    struct Row: Identifiable, Equatable {
        var catalog: CatalogEntry
        var status: Status

        var id: String { catalog.name }

        var canCheck: Bool { CatalogUpdatesModel.isCheckable(catalog) && status != .checking }

        var canUpdate: Bool {
            if case .updateAvailable = status { true } else { false }
        }
    }

    private(set) var records: [String: CatalogCheckRecord]
    private(set) var lastCheckedAt: Date?
    /// The catalogs still waiting for their result in the running check.
    private(set) var pending: Set<String> = []
    private(set) var isChecking = false
    /// The open Review update sheet.
    private(set) var review: CatalogUpdateReview?
    var presentedError: PresentedError?

    @ObservationIgnored private(set) weak var setup: SetupModel?
    @ObservationIgnored private let store: AppStateStore?
    @ObservationIgnored private var checkTask: Task<Void, Never>?
    @ObservationIgnored private let key: String

    init(setup: SetupModel, store: AppStateStore?) {
        self.setup = setup
        self.store = store
        key = setup.root.path
        let saved = store?.state.catalogChecks[key] ?? SetupCatalogChecks()
        records = saved.results
        lastCheckedAt = saved.lastCheckedAt
    }

    // MARK: State

    /// The saved config's catalogs. Updates never look at the draft.
    var catalogs: [CatalogEntry] {
        guard case let .valid(_, _, _, summary) = setup?.snapshot?.config else {
            return []
        }
        return summary.catalogs
    }

    var rows: [Row] {
        catalogs.map { Row(catalog: $0, status: status(of: $0)) }
    }

    /// The catalogs Check for updates covers: remote ones that are not pinned to a commit.
    var checkableCatalogs: [String] {
        catalogs.filter(Self.isCheckable).map(\.name)
    }

    /// The revisions an update of every outdated catalog would install.
    var availableUpdates: [ReviewedRevision] {
        rows.compactMap { row in
            guard case let .updateAvailable(record) = row.status, let latest = record.latest else {
                return nil
            }
            return ReviewedRevision(catalog: row.catalog.name, commit: latest)
        }
    }

    /// True while the setup's draft keeps updates from starting.
    var isBlockedByDraft: Bool { setup?.hasPendingChanges ?? false }

    /// The running check, for stage and progress display.
    var operation: OperationRunner.Operation? {
        guard isChecking, let setup, let current = setup.operations.current, current.setup == setup.id else {
            return nil
        }
        return current
    }

    func status(of catalog: CatalogEntry) -> Status {
        switch catalog.sourceKind {
        case .local:
            return .localFiles
        case .git(_, _, commitRef: true):
            return .pinned(commit: catalog.gitRef)
        case .git, nil:
            break
        }

        if pending.contains(catalog.name) {
            return .checking
        }
        guard let record = records[catalog.name], record.applies(to: catalog) else {
            return .unchecked
        }

        switch record.outcome {
        case .upToDate: return .upToDate(checkedAt: record.checkedAt)
        case .updateAvailable: return .updateAvailable(record)
        case .pinned: return .pinned(commit: record.installed ?? catalog.gitRef)
        case .localFiles: return .localFiles
        case .failed:
            return .failed(
                message: record.failure ?? String(localized: "The check failed."), lastKnown: record.installed,
                checkedAt: record.checkedAt)
        }
    }

    nonisolated static func isCheckable(_ catalog: CatalogEntry) -> Bool {
        switch catalog.sourceKind {
        case .local, .git(_, _, commitRef: true): false
        case .git, nil: true
        }
    }

    // MARK: Checking

    /// Checks every remote catalog that is not pinned to a commit.
    func checkAll() async {
        await check(checkableCatalogs)
    }

    /// Checks `names` one at a time. A failure is recorded for its catalog and the others go on.
    /// Canceling leaves the catalogs not checked yet as they were.
    func check(_ names: [String]) async {
        guard let setup, !isChecking else {
            return
        }

        let entries = names.compactMap { name in catalogs.first { $0.name == name } }.filter(Self.isCheckable)
        guard !entries.isEmpty else {
            return
        }

        isChecking = true
        pending = Set(entries.map(\.name))
        let session = setup.session
        let task = Task { @MainActor in
            do {
                try await setup.operations.run(String(localized: "Checking for catalog updates"), setup: setup.id) {
                    progress in
                    for entry in entries {
                        try Task.checkCancellation()
                        let record: CatalogCheckRecord
                        do {
                            let check = try await session.checkCatalogUpdate(catalog: entry.name, progress: progress)
                            record = Self.record(of: check, for: entry)
                        } catch where error.isCancellation {
                            throw error
                        } catch {
                            record = self.failureRecord(EngineError(error), for: entry)
                        }
                        self.save(record, for: entry.name)
                    }
                }
            } catch {
                // Canceled: the catalogs not checked yet keep their earlier results.
            }

            self.pending = []
            self.isChecking = false
        }
        checkTask = task
        await task.value
        checkTask = nil
    }

    /// Stops the running check after the catalog being checked.
    func cancelCheck() {
        checkTask?.cancel()
    }

    private static func record(of check: CatalogUpdateCheck, for entry: CatalogEntry) -> CatalogCheckRecord {
        let outcome: CatalogCheckRecord.Outcome =
            switch check.freshness {
            case .outdated: .updateAvailable
            case .current: .upToDate
            case .pinned: .pinned
            case .local: .localFiles
            }
        return CatalogCheckRecord(
            outcome: outcome, checkedAt: Date(), source: entry.source, gitRef: entry.gitRef, installed: check.commit,
            latest: check.latest, added: check.changes.added.map(CheckedItem.init),
            removed: check.changes.removed.map(CheckedItem.init))
    }

    private func failureRecord(_ error: EngineError, for entry: CatalogEntry) -> CatalogCheckRecord {
        let previous = records[entry.name].flatMap { $0.applies(to: entry) ? $0 : nil }
        return CatalogCheckRecord(
            outcome: .failed, checkedAt: Date(), source: entry.source, gitRef: entry.gitRef,
            installed: previous?.installed, latest: nil, failure: error.message)
    }

    private func save(_ record: CatalogCheckRecord, for name: String) {
        records[name] = record
        pending.remove(name)
        lastCheckedAt = record.checkedAt
        persist()
    }

    private func persist() {
        let checks = SetupCatalogChecks(lastCheckedAt: lastCheckedAt, results: records)
        do {
            try store?.update { $0.catalogChecks[key] = checks }
            if let saved = store?.state.catalogChecks[key] {
                records = saved.results
                lastCheckedAt = saved.lastCheckedAt
            }
        } catch {
            presentedError = PresentedError(title: String(localized: "Could not save Ambit's settings."), error: error)
        }
    }

    // MARK: Updating

    /// Opens the Review update sheet for `names`, or for every outdated catalog when `nil`.
    /// Does nothing while the draft has unapplied edits; those must be applied or discarded first.
    func reviewUpdate(_ names: [String]? = nil) async {
        guard let setup, review == nil else {
            return
        }

        await setup.refresh()
        guard !setup.hasPendingChanges else {
            return
        }

        let revisions = availableUpdates.filter { names?.contains($0.catalog) ?? true }
        guard !revisions.isEmpty else {
            return
        }

        let review = CatalogUpdateReview(setup: setup, revisions: revisions) { [weak self] installed in
            self?.updateInstalled(installed)
        }
        self.review = review
        await review.load()
    }

    /// Closes the sheet, canceling a review or apply that can still be canceled.
    func closeReview() {
        guard let review else {
            return
        }

        if review.isRunning {
            guard review.canCancel else {
                return
            }
            review.cancel()
        }
        self.review = nil
    }

    /// The installed catalogs now sit at the reviewed commits, which the check found to be the
    /// latest.
    private func updateInstalled(_ revisions: [ReviewedRevision]) {
        for revision in revisions {
            guard var record = records[revision.catalog] else {
                continue
            }
            record.outcome = .upToDate
            record.installed = revision.commit
            record.latest = revision.commit
            record.added = []
            record.removed = []
            records[revision.catalog] = record
        }
        persist()
    }
}

/// One Review update sheet: plan the update with the checked commits, show it, and apply that
/// plan. When the setup changed after the review, the plan is refreshed and shown again instead
/// of applying different work.
@MainActor
@Observable
final class CatalogUpdateReview {
    enum Phase: Equatable {
        case reviewing
        /// `refreshed` is true when this plan replaced one that could no longer be applied.
        case ready(ReviewSummary, refreshed: Bool)
        case reviewFailed(EngineError)
        case applying
        case finished(ReviewModel.Outcome)
    }

    let revisions: [ReviewedRevision]
    private(set) var phase: Phase = .reviewing

    @ObservationIgnored private weak var setup: SetupModel?
    @ObservationIgnored private let onInstalled: @MainActor ([ReviewedRevision]) -> Void
    @ObservationIgnored private var handle: ReviewHandle?
    @ObservationIgnored private var task: Task<Void, Never>?

    init(
        setup: SetupModel, revisions: [ReviewedRevision],
        onInstalled: @escaping @MainActor ([ReviewedRevision]) -> Void
    ) {
        self.setup = setup
        self.revisions = revisions
        self.onInstalled = onInstalled
    }

    var operation: OperationRunner.Operation? {
        guard isRunning, let setup, let current = setup.operations.current, current.setup == setup.id else {
            return nil
        }
        return current
    }

    var canCancel: Bool {
        switch phase {
        case .reviewing: true
        case .applying: operation?.isCancellable ?? true
        case .ready, .reviewFailed, .finished: false
        }
    }

    var isRunning: Bool { phase == .reviewing || phase == .applying }

    var didInstall: Bool {
        if case .finished(.installed) = phase { true } else { false }
    }

    /// Plans the update. Never writes.
    func load(refreshed: Bool = false) async {
        guard let setup else {
            return
        }

        phase = .reviewing
        handle = nil
        let session = setup.session
        let revisions = revisions
        await track {
            do {
                let handle = try await setup.operations.run(String(localized: "Reviewing catalog update"), setup: setup.id) {
                    progress in
                    try await session.reviewCatalogUpdates(revisions, progress: progress)
                }
                self.handle = handle
                self.phase = .ready(handle.summary, refreshed: refreshed)
            } catch {
                self.phase = error.isCancellation ? .finished(.canceled) : .reviewFailed(EngineError(error))
            }
        }
    }

    /// Installs the reviewed plan. A stale plan is refreshed and shown again.
    func apply() async {
        guard let setup, let handle, case let .ready(summary, _) = phase, summary.canApply else {
            return
        }

        phase = .applying
        let session = setup.session
        await run(String(localized: "Updating catalogs"), setup: setup) { progress in
            try await session.apply(handle, progress: progress)
        }
    }

    /// Plans the update again after it failed or was canceled. A plain retry of the installation
    /// would install the saved lock's revisions, which an update that failed may not have
    /// replaced yet.
    func reviewAgain() async {
        guard !isRunning else {
            return
        }
        await load(refreshed: true)
    }

    func cancel() {
        guard canCancel else {
            return
        }
        task?.cancel()
    }

    private func run(
        _ title: String, setup: SetupModel,
        _ body: @escaping @MainActor (_ progress: @escaping ProgressHandler) async throws -> ApplyOutcome
    ) async {
        await track {
            let outcome: ReviewModel.Outcome
            do {
                switch try await setup.operations.run(title, setup: setup.id, body) {
                case let .installed(summary): outcome = .installed(summary)
                case let .notFullyInstalled(saved, error): outcome = .notFullyInstalled(saved: saved, error: error)
                }
            } catch {
                let error = EngineError(error)
                switch error {
                case .staleReview: outcome = .stale(error)
                case .canceled: outcome = .canceled
                default: outcome = .saveFailed(error)
                }
            }

            self.handle = nil
            // The draft is clean (updates require it), so the retry mode, which never touches the
            // draft, records the outcome correctly.
            await setup.applyFinished(outcome, mode: .retryInstall)

            switch outcome {
            case .stale:
                await self.load(refreshed: true)
            case .installed:
                self.onInstalled(self.revisions)
                self.phase = .finished(outcome)
            case .notFullyInstalled, .saveFailed, .canceled:
                self.phase = .finished(outcome)
            }
        }
    }

    private func track(_ body: @escaping @MainActor () async -> Void) async {
        let task = Task { @MainActor in await body() }
        self.task = task
        await task.value
        if self.task == task {
            self.task = nil
        }
    }
}
