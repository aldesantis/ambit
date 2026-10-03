// Runs engine operations that fetch, plan or write one at a time, app-wide. The engine also
// holds a per-setup lock, but the shared catalog cache and the UI's progress display need the
// app to serialize everything, not just operations on the same root. Read-only calls such as
// `snapshot()` and `browse` do not go through here.

import Foundation
import Observation

@MainActor
@Observable
final class OperationRunner {
    struct Operation: Identifiable, Equatable {
        let id: UUID
        var title: String
        var setup: SetupID?
        var progress: ProgressEvent?
        /// False once the operation reports a stage that writes. Canceling after that point would
        /// leave a partial install, so the UI disables Cancel instead.
        var isCancellable = true
    }

    /// The running operation. Queued operations are not listed.
    private(set) var current: Operation?

    @ObservationIgnored private var tail: Task<Void, Never>?

    var isBusy: Bool { current != nil }

    /// Runs `body` after every operation queued before it finishes.
    ///
    /// Canceling the calling task cancels `body`, or skips it while it is still queued; the
    /// engine then throws `EngineError.canceled` or the queue throws `CancellationError`.
    func run<T: Sendable>(
        _ title: String, setup: SetupID? = nil,
        _ body: @escaping @MainActor (_ progress: @escaping ProgressHandler) async throws -> T
    ) async throws -> T {
        let previous = tail
        let id = UUID()
        let task = Task<T, any Error> { @MainActor in
            await previous?.value
            try Task.checkCancellation()

            self.current = Operation(id: id, title: title, setup: setup)
            defer { self.current = nil }
            return try await body { event in
                self.report(event, for: id)
            }
        }
        tail = Task { _ = await task.result }

        return try await withTaskCancellationHandler {
            try await task.value
        } onCancel: {
            task.cancel()
        }
    }

    private func report(_ event: ProgressEvent, for id: UUID) {
        guard current?.id == id else {
            return
        }

        current?.progress = event
        if event.stage.writes {
            current?.isCancellable = false
        }
    }
}

extension Stage {
    /// True for the stages that change files in the setup root.
    var writes: Bool {
        switch self {
        case .savingConfig, .writingFiles, .removingFiles, .writingRecords: true
        case .loadingCatalogs, .fetching, .resolving, .planning, .checkingOwnership: false
        }
    }
}

extension Error {
    /// True for a Swift task cancellation and for the engine's own cancellation error.
    var isCancellation: Bool {
        self is CancellationError || (self as? EngineError) == .canceled
    }
}
