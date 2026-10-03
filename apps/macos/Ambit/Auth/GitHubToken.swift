import Foundation

/// A GitHub OAuth access token.
///
/// Every textual form of the value (`description`, `debugDescription`, `dump`, string
/// interpolation, mirrors) is redacted, so a token passed to a log or an error message by mistake
/// prints a placeholder. Read `value` only to hand the token to the Keychain, the engine, or an
/// `Authorization` header.
struct GitHubToken: Sendable, Equatable {
    let value: String

    init(_ value: String) {
        self.value = value
    }
}

extension GitHubToken: CustomStringConvertible, CustomDebugStringConvertible, CustomReflectable {
    static let redacted = "<redacted>"

    var description: String { Self.redacted }
    var debugDescription: String { "GitHubToken(\(Self.redacted))" }
    var customMirror: Mirror { Mirror(self, children: [], displayStyle: .struct) }
}

/// The one account the app remembers: the token and the username it belongs to.
struct GitHubAccount: Sendable, Equatable {
    var username: String
    var token: GitHubToken
}
