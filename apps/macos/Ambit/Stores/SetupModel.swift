// State of one setup root: the saved config as last read, the in-memory draft, the open review
// and the last installation failure. Each feature area extends this type in its own
// `SetupModel+<Area>.swift` file (for example `SetupModel+Catalogs.swift`) and keeps its stored
// state in a separate area model that this type owns, so areas never edit each other's files.
// Areas read `configSummary` and `draftText`, and change the config only through `stage(_:)`.
//
// External edits: the draft remembers the config it started from (`DraftModel.base`). Every
// re-read compares the file on disk with it. A clean setup simply takes the new file. With
// unapplied edits the app never merges or overwrites: it asks the user to reload (dropping the
// draft) or keep editing, and refuses to review until the draft matches the file again.

import Foundation
import Observation

@MainActor
@Observable
final class SetupModel {
    /// The status shown next to a setup's name.
    enum Badge: Equatable {
        case unconfigured
        case pendingChanges
        case installing
        case installed
        case notFullyInstalled
        case folderUnavailable
        case error
    }

    /// The steps of the flow that creates a config for a root without one.
    enum NewSetupStep: Equatable {
        case tools
        case catalog
        case capabilities
    }

    let id: SetupID
    let root: URL
    @ObservationIgnored let engine: any EngineService
    @ObservationIgnored let session: any SetupSessionService
    @ObservationIgnored let operations: OperationRunner

    private(set) var snapshot: SetupSnapshot?
    /// Why the last `refresh` could not read the setup root.
    private(set) var loadError: EngineError?
    private(set) var isLoading = false
    private(set) var draft: DraftModel?
    /// The open review sheet.
    private(set) var review: ReviewModel?
    /// Set when the last apply or retry saved the config but could not install all of it.
    private(set) var installFailure: EngineError?
    /// A snapshot whose config differs from the draft's base, waiting for the user's choice.
    private(set) var externalChange: SetupSnapshot?
    /// An edit or review that failed outside the review sheet.
    var presentedError: PresentedError?

    var newSetupStep: NewSetupStep = .tools
    /// The harness names checked in the new-setup flow, before the draft exists.
    var newSetupTools: Set<String> = []

    /// A file change the user chose to keep editing past. It is not asked about again on focus,
    /// but review stays blocked until the user reloads.
    @ObservationIgnored private var acknowledgedChange: ConfigState?
    @ObservationIgnored private var reviewWaiter: CheckedContinuation<Bool, Never>?

    init(id: SetupID, root: URL, engine: any EngineService, operations: OperationRunner) {
        self.id = id
        self.root = root
        self.engine = engine
        self.operations = operations
        session = engine.openSetup(root: root.path)
    }

    var displayName: String {
        switch id {
        case .personal: String(localized: "Personal setup")
        case .project: root.lastPathComponent
        }
    }

    /// True while the draft differs from the saved config.
    var hasPendingChanges: Bool { draft?.isDirty ?? false }

    /// The draft text to plan and browse with. `nil` when there are no pending changes, which
    /// means "use the saved config".
    var draftText: String? {
        guard let draft, draft.isDirty else {
            return nil
        }
        return draft.text
    }

    /// The config as the user currently sees it: the draft when there is one, otherwise the saved
    /// config. `nil` when the setup has no usable config and no new-setup draft.
    var configSummary: ConfigSummary? {
        if let draft {
            return draft.summary
        }
        if case let .valid(_, _, _, summary) = snapshot?.config {
            return summary
        }
        return nil
    }

    /// The config file name edits are made against.
    var fileName: String {
        if let draft {
            return draft.fileName
        }
        if case let .valid(_, fileName, _, _) = snapshot?.config {
            return fileName
        }
        return DraftModel.newFileName
    }

    /// True after the user kept editing past an external change. Review stays blocked.
    var isDraftOutdated: Bool { hasPendingChanges && acknowledgedChange != nil }

    var isApplying: Bool {
        operations.current?.setup == id && review?.phase == .applying
    }

    var badge: Badge? {
        if loadError != nil {
            return .error
        }

        guard let snapshot else {
            return nil
        }

        if isApplying {
            return .installing
        }
        if hasPendingChanges {
            return .pendingChanges
        }
        switch snapshot.config {
        case .missing: return .unconfigured
        case .ambiguous, .invalid: return .error
        case .valid: return installFailure == nil ? .installed : .notFullyInstalled
        }
    }

    // MARK: Reading

    /// Re-reads the config. Never fetches catalogs. With unapplied edits and a changed file, asks
    /// the user through `externalChange` unless they already chose to keep editing past it.
    func refresh() async {
        _ = await read(forcePrompt: false)
    }

    /// Re-reads the config before a review. Returns false when the file no longer matches the
    /// draft; `externalChange` is then set and the review must not start.
    func prepareForReview() async -> Bool {
        await read(forcePrompt: true)
    }

    /// Takes the changed file and drops the draft.
    func reloadDiscardingDraft() {
        guard let change = externalChange else {
            return
        }

        clearDraft()
        snapshot = change
        externalChange = nil
    }

    /// Keeps the draft after an external change. The change is not asked about again on focus.
    func keepDraftAfterExternalChange() {
        acknowledgedChange = externalChange?.config
        externalChange = nil
    }

    private func read(forcePrompt: Bool) async -> Bool {
        isLoading = true
        defer { isLoading = false }

        let next: SetupSnapshot
        do {
            next = try await session.snapshot()
            loadError = nil
        } catch {
            loadError = EngineError(error)
            return false
        }

        guard let draft, draft.isDirty, next.config != draft.base else {
            if draft?.isDirty == false {
                // A clean draft only holds edits that cancel out. Take the file as it is now.
                self.draft = nil
            }
            snapshot = next
            acknowledgedChange = nil
            externalChange = nil
            return true
        }

        if !forcePrompt, acknowledgedChange == next.config {
            return false
        }

        externalChange = next
        return false
    }

    // MARK: Editing

    /// Stages `edits` in the draft, starting one from the saved config if needed.
    ///
    /// Throws the engine's error when an edit does not apply; the draft is then unchanged.
    /// Throws `EngineError.config` when the setup has no valid config and no new-setup draft.
    func stage(_ edits: [ConfigEdit]) throws {
        if let draft {
            try draft.stage(edits)
            return
        }

        guard let config = snapshot?.config, case .valid = config else {
            throw EngineError.config(
                message: String(localized: "This setup has no configuration to edit."), detail: [], path: nil,
                line: nil)
        }

        let draft = DraftModel(editing: config, engine: engine)
        try draft.stage(edits)
        self.draft = draft
    }

    /// Starts, or restarts with other tools, the draft of a new config.
    func startNewSetup(harnesses: [String]) throws {
        if let draft, draft.isNew {
            try draft.replaceNewHarnesses(harnesses)
            return
        }

        draft = try DraftModel(newWith: harnesses, engine: engine)
    }

    /// Drops the draft. Nothing on disk changes.
    func discardChanges() {
        clearDraft()
    }

    private func clearDraft() {
        draft = nil
        acknowledgedChange = nil
        newSetupStep = .tools
        newSetupTools = []
    }

    // MARK: Review and apply

    /// Opens the review sheet for the draft. Does nothing when the file changed on disk; the
    /// setup then asks about the external change instead.
    func startReview() async {
        guard review == nil, await prepareForReview() else {
            return
        }

        let review = ReviewModel(setup: self, mode: .apply)
        self.review = review
        await review.load()
    }

    /// Reviews and applies the draft, waiting until the sheet closes. Returns true only when the
    /// changes were installed. Used before an action that must not lose the draft, such as
    /// switching setups; the sheet closes by itself after a successful install.
    func reviewAndApply() async -> Bool {
        await startReview()
        guard let review else {
            return false
        }

        if case .finished = review.phase, !review.didInstall {
            // Nothing to wait for when the review itself was canceled.
            closeReview()
            return false
        }

        return await withCheckedContinuation { continuation in
            if let previous = reviewWaiter {
                previous.resume(returning: false)
            }
            reviewWaiter = continuation
        }
    }

    /// Opens the sheet in retry mode and installs the saved config again.
    func retryInstall() async {
        guard review == nil else {
            return
        }

        let review = ReviewModel(setup: self, mode: .retryInstall)
        self.review = review
        await review.retryInstall()
    }

    /// Closes the review sheet, canceling a review or apply that can still be canceled.
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
        reviewWaiter?.resume(returning: review.didInstall)
        reviewWaiter = nil
    }

    /// Records what an apply or retry left behind and re-reads the setup. A retry installs the
    /// saved config only, so it never touches the draft.
    func applyFinished(_ outcome: ReviewModel.Outcome, mode: ReviewModel.Mode) async {
        switch outcome {
        case .installed:
            if mode == .apply {
                clearDraft()
            }
            installFailure = nil
        case let .notFullyInstalled(saved, error):
            if saved, mode == .apply {
                clearDraft()
            }
            installFailure = error
        case .saveFailed, .stale, .canceled:
            break
        }

        await refresh()

        if case .installed = outcome, reviewWaiter != nil {
            closeReview()
        }
    }
}
