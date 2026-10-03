import Foundation

/// Mirrors the FFI `EngineError`. Messages are already redacted by the engine.
enum EngineError: Error, Sendable, Hashable {
    case config(message: String, detail: [String], path: String?, line: UInt32?)
    case resolution(message: String, detail: [String])
    case network(message: String, detail: [String], kind: NetworkKind)
    case ownershipConflict(message: String, detail: [String], path: String)
    /// The setup changed after the review. The user must review again.
    case staleReview(message: String, detail: [String])
    /// Another Ambit operation, possibly the CLI, holds the setup's operation lock.
    case busy(message: String, detail: [String])
    case canceled
    case `internal`(message: String, detail: [String])
}

enum NetworkKind: Sendable, Hashable {
    case notCached
    case offline
    case authRequired
    case accessDenied(sso: Bool)
    case notFound
    case other
}

extension EngineError: LocalizedError {
    var message: String {
        switch self {
        case let .config(message, _, _, _), let .resolution(message, _), let .network(message, _, _),
            let .ownershipConflict(message, _, _), let .staleReview(message, _), let .busy(message, _),
            let .internal(message, _):
            message
        case .canceled:
            String(localized: "The operation was canceled.")
        }
    }

    var detail: [String] {
        switch self {
        case let .config(_, detail, _, _), let .resolution(_, detail), let .network(_, detail, _),
            let .ownershipConflict(_, detail, _), let .staleReview(_, detail), let .busy(_, detail),
            let .internal(_, detail):
            detail
        case .canceled:
            []
        }
    }

    var errorDescription: String? { message }
}
