// The GitHub account as views see it: a state and a username, never the token. The token goes
// from the device flow straight to the Keychain and the engine and is not kept here.

import Foundation
import Observation

@MainActor
@Observable
final class GitHubAccountModel {
    enum State: Equatable {
        case signedOut
        /// Asking GitHub for a code.
        case requestingCode
        case awaitingUser(code: String, verificationURL: URL, expiresAt: Date)
        case signedIn(username: String)
        case error(GitHubAuthError)
    }

    private(set) var state: State = .signedOut

    /// False when the build has no OAuth client ID.
    var isAvailable: Bool { services.clientID != nil }

    var username: String? {
        if case let .signedIn(username) = state { username } else { signedInUsername }
    }

    /// True while a sign-in started by `signIn()` has not finished.
    var isSigningIn: Bool {
        switch state {
        case .requestingCode, .awaitingUser: true
        default: false
        }
    }

    /// The OAuth app's GitHub page, where a user grants it access to an SSO organization.
    var authorizationSettingsURL: URL? {
        services.clientID.map { URL(string: "https://github.com/settings/connections/applications/\($0)")! }
    }

    @ObservationIgnored private let services: GitHubServices
    @ObservationIgnored private let engine: any EngineService
    /// The username of the stored account, kept so a failed or canceled sign-in returns to it.
    @ObservationIgnored private var signedInUsername: String?
    @ObservationIgnored private(set) var flow: Task<Void, Never>?

    init(services: GitHubServices, engine: any EngineService) {
        self.services = services
        self.engine = engine
    }

    /// Reads the stored account and hands its token to the engine. Makes no network request.
    func restore() async {
        do {
            guard let account = try services.tokenStore.load() else {
                return
            }
            await engine.setGitHubToken(account.token.value)
            signedInUsername = account.username
            if !isSigningIn {
                state = .signedIn(username: account.username)
            }
        } catch {
            state = .error(Self.authError(error))
        }
    }

    /// Starts the device flow. Also the "sign in again" action: a new account replaces the
    /// stored one only once it is authorized.
    func signIn() {
        guard let clientID = services.clientID else {
            state = .error(.unavailable)
            return
        }

        flow?.cancel()
        state = .requestingCode
        flow = Task { await self.run(clientID: clientID) }
    }

    /// Stops polling and returns to the state before `signIn()`.
    func cancelSignIn() {
        flow?.cancel()
        flow = nil
        if isSigningIn {
            state = restingState
        }
    }

    /// Leaves an error state.
    func dismissError() {
        if case .error = state {
            state = restingState
        }
    }

    /// Deletes the stored account and clears the engine's token. Installed capabilities,
    /// configurations and cached catalogs stay.
    func signOut() async {
        flow?.cancel()
        flow = nil
        do {
            try services.tokenStore.delete()
        } catch {
            state = .error(Self.authError(error))
            return
        }
        await engine.setGitHubToken(nil)
        signedInUsername = nil
        state = .signedOut
    }

    private var restingState: State {
        signedInUsername.map { .signedIn(username: $0) } ?? .signedOut
    }

    private func run(clientID: String) async {
        let client = services.client
        do {
            let authorization = try await client.requestAuthorization(clientID: clientID)
            try Task.checkCancellation()
            state = .awaitingUser(
                code: authorization.userCode, verificationURL: authorization.verificationURL,
                expiresAt: authorization.expiresAt)

            let token = try await client.waitForToken(clientID: clientID, authorization: authorization)
            let username = try await client.username(token: token)
            try Task.checkCancellation()

            try services.tokenStore.save(GitHubAccount(username: username, token: token))
            await engine.setGitHubToken(token.value)
            signedInUsername = username
            state = .signedIn(username: username)
        } catch is CancellationError {
            // `cancelSignIn` already restored the state.
        } catch {
            guard !Task.isCancelled else {
                return
            }
            state = .error(Self.authError(error))
        }
    }

    private static func authError(_ error: any Error) -> GitHubAuthError {
        (error as? GitHubAuthError) ?? .unexpected(String(describing: type(of: error)))
    }
}
