import Foundation

/// What to tell the user when the engine reports that a GitHub repository could not be read.
/// Catalog views show `message` with Retry, plus Sign In Again when `offersSignIn` is set.
struct GitHubAccessExplanation: Equatable {
    var title: String
    var message: String
    var offersSignIn: Bool
    /// Where to grant Ambit access to an organization that enforces SAML single sign-on.
    var authorizationURL: URL?

    /// `nil` for network failures that are not about access.
    init?(kind: NetworkKind, signedInAs username: String?, authorizationURL: URL? = nil) {
        switch kind {
        case .authRequired:
            title = String(localized: "Sign-in required")
            message =
                if let username {
                    String(
                        localized: """
                            GitHub did not accept the sign-in for \(username). It may have been revoked. \
                            Sign in again, then retry.
                            """)
                } else {
                    String(
                        localized: """
                            This repository is private or does not exist. Sign in with GitHub to use private \
                            catalogs, then retry.
                            """)
                }
            offersSignIn = true
            self.authorizationURL = nil
        case .accessDenied(sso: true):
            title = String(localized: "Organization sign-on required")
            message = String(
                localized: """
                    The organization that owns this repository requires SAML single sign-on. Authorize \
                    Ambit for that organization on GitHub, then retry or sign in again.
                    """)
            offersSignIn = true
            self.authorizationURL = authorizationURL
        case .accessDenied(sso: false):
            title = String(localized: "No access to this repository")
            message =
                if let username {
                    String(
                        localized: """
                            The GitHub account \(username) cannot read this repository. Ask its owners for \
                            access. If the organization restricts OAuth apps, an owner must approve Ambit first.
                            """)
                } else {
                    String(
                        localized: """
                            GitHub refused access to this repository. Sign in with an account that can read it, \
                            then retry.
                            """)
                }
            offersSignIn = true
            self.authorizationURL = authorizationURL
        case .notFound:
            title = String(localized: "Repository not found")
            message =
                if let username {
                    String(
                        localized: """
                            GitHub found no repository at this address that \(username) can see. Check the \
                            source, or sign in with an account that has access.
                            """)
                } else {
                    String(
                        localized: """
                            GitHub found no repository at this address. If it is private, sign in with GitHub \
                            and retry.
                            """)
                }
            offersSignIn = true
            self.authorizationURL = nil
        case .notCached, .offline, .other:
            return nil
        }
    }
}
