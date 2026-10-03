// Runs blocking FFI calls off the main actor. Reads share a concurrent queue; mutations go
// through one serial queue for the whole app, so two of them never interleave even if a caller
// forgets to serialize. Long operations get a `CancelToken` tied to Swift task cancellation and a
// progress listener that reports on the main actor.

import AmbitEngine
import Foundation

final class LiveEngineExecutor: Sendable {
    enum Kind: Sendable {
        /// Reads files only (snapshot, browse, status). Runs concurrently with other reads.
        case read
        /// Fetches, writes, or changes engine state. One at a time, in submission order.
        case mutation
    }

    private let reads = DispatchQueue(
        label: "com.nebulab.ambit.engine.reads", qos: .userInitiated, attributes: .concurrent)
    private let mutations = DispatchQueue(label: "com.nebulab.ambit.engine.mutations", qos: .userInitiated)

    /// Runs `body` on the queue for `kind` and translates FFI errors to the app's `EngineError`.
    func run<T: Sendable>(_ kind: Kind, _ body: @escaping @Sendable () throws -> T) async throws -> T {
        let queue = kind == .read ? reads : mutations
        return try await withCheckedThrowingContinuation { continuation in
            queue.async {
                continuation.resume(with: Result { try LiveEngineMapping.translatingErrors(body) })
            }
        }
    }

    /// Like `run`, for an export that takes a cancel token and a progress listener. Cancelling the
    /// calling task cancels the token; the engine then stops at its next check and throws
    /// `canceled`, or finishes when it has already started writing.
    func operation<T: Sendable>(
        _ kind: Kind, progress: ProgressHandler?,
        _ body: @escaping @Sendable (AmbitEngine.CancelToken, (any AmbitEngine.ProgressListener)?) throws -> T
    ) async throws -> T {
        let token = AmbitEngine.CancelToken()
        let listener = progress.map(LiveProgressListener.init)
        return try await withTaskCancellationHandler {
            if Task.isCancelled {
                token.cancel()
            }
            return try await run(kind) { try body(token, listener) }
        } onCancel: {
            token.cancel()
        }
    }
}

/// Forwards engine progress to a handler on the main actor, in the order the engine reported it.
final class LiveProgressListener: AmbitEngine.ProgressListener {
    private let handler: ProgressHandler

    init(_ handler: @escaping ProgressHandler) {
        self.handler = handler
    }

    func onProgress(event: AmbitEngine.ProgressEvent) {
        let converted = LiveEngineMapping.progressEvent(event)
        let handler = handler
        // The main queue is FIFO, unlike unstructured tasks, so events never arrive reordered.
        DispatchQueue.main.async {
            MainActor.assumeIsolated { handler(converted) }
        }
    }
}
