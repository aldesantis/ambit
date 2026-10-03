// One review sheet: plan the draft, show what Apply would do, apply exactly that plan, and
// report the outcome. A review never writes. Apply hands the engine the reviewed plan, whose
// fingerprint is the final guard against changes made after the review (`staleReview`).
//
// Outcomes and what they leave behind:
// - installed: the config is saved and installed; the draft is gone.
// - notFullyInstalled: the config is saved (the draft is gone) but installation failed;
//   the setup offers Retry installation.
// - saveFailed: nothing was saved; the draft is kept.
// - stale: the setup changed after the review; nothing was saved; the user reviews again.
// - canceled: canceled before any write; nothing was saved.

import Foundation
import Observation

@MainActor
@Observable
final class ReviewModel {
    enum Mode: Equatable {
        /// Review the draft, then apply it.
        case apply
        /// Install the saved config again after a failed installation. There is nothing to review.
        case retryInstall
    }

    enum Phase: Equatable {
        case reviewing
        case ready(ReviewSummary)
        case reviewFailed(EngineError)
        case applying
        case finished(Outcome)
    }

    enum Outcome: Equatable {
        case installed(InstallSummary)
        case notFullyInstalled(saved: Bool, error: EngineError)
        case saveFailed(EngineError)
        case stale(EngineError)
        case canceled
    }

    let mode: Mode
    private(set) var phase: Phase

    @ObservationIgnored private weak var setup: SetupModel?
    @ObservationIgnored private var handle: ReviewHandle?
    @ObservationIgnored private var task: Task<Void, Never>?

    init(setup: SetupModel, mode: Mode) {
        self.setup = setup
        self.mode = mode
        phase = mode == .apply ? .reviewing : .applying
    }

    /// The running operation of this review's setup, for stage and progress display.
    var operation: OperationRunner.Operation? {
        guard let setup, let current = setup.operations.current, current.setup == setup.id else {
            return nil
        }
        return current
    }

    /// True while reviewing, and while applying until the engine starts writing.
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

    /// Plans the setup's draft, or its saved config when there is no draft.
    func load() async {
        guard let setup else {
            return
        }

        phase = .reviewing
        handle = nil
        let draftText = setup.draftText
        let session = setup.session
        await track {
            do {
                let handle = try await setup.operations.run(String(localized: "Reviewing changes"), setup: setup.id) {
                    progress in
                    try await session.review(draftText: draftText, progress: progress)
                }
                self.handle = handle
                self.phase = .ready(handle.summary)
            } catch {
                self.phase = error.isCancellation ? .finished(.canceled) : .reviewFailed(EngineError(error))
            }
        }
    }

    /// Saves and installs the reviewed plan. Does nothing unless the review can be applied.
    func apply() async {
        guard let setup, let handle, case let .ready(summary) = phase, summary.canApply else {
            return
        }

        phase = .applying
        let session = setup.session
        await run(String(localized: "Applying changes"), setup: setup) { progress in
            try await session.apply(handle, progress: progress)
        }
    }

    /// Installs the saved config again.
    func retryInstall() async {
        guard let setup else {
            return
        }

        phase = .applying
        let session = setup.session
        await run(String(localized: "Retrying installation"), setup: setup) { progress in
            try await session.retryInstall(progress: progress)
        }
    }

    /// Re-reads the setup and reviews again, after a stale review or a canceled apply. Returns
    /// false when the config changed on disk and the setup is asking the user what to do.
    func reviewAgain() async -> Bool {
        guard let setup, await setup.prepareForReview() else {
            return false
        }

        await load()
        return true
    }

    /// Cancels the running review or apply. Ignored once apply is writing.
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
            let outcome: Outcome
            do {
                switch try await setup.operations.run(title, setup: setup.id, body) {
                case let .installed(summary):
                    outcome = .installed(summary)
                case let .notFullyInstalled(saved, error):
                    outcome = .notFullyInstalled(saved: saved, error: error)
                }
            } catch let error as EngineError {
                switch error {
                case .staleReview: outcome = .stale(error)
                case .canceled: outcome = .canceled
                default: outcome = .saveFailed(error)
                }
            } catch {
                outcome = error.isCancellation ? .canceled : .saveFailed(EngineError(error))
            }

            self.handle = nil
            self.phase = .finished(outcome)
            await setup.applyFinished(outcome, mode: self.mode)
        }
    }

    /// Runs `body` as the cancellable task of this review and waits for it.
    private func track(_ body: @escaping @MainActor () async -> Void) async {
        let task = Task { @MainActor in await body() }
        self.task = task
        await task.value
        if self.task == task {
            self.task = nil
        }
    }
}

extension EngineError {
    /// Wraps an error that did not come from the engine.
    init(_ error: any Error) {
        if let error = error as? EngineError {
            self = error
        } else if error.isCancellation {
            self = .canceled
        } else {
            self = .internal(message: error.localizedDescription, detail: [])
        }
    }
}
