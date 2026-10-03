// GitHub's OAuth device flow for an OAuth app:
// https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/authorizing-oauth-apps#device-flow
//
// No client secret exists: the client ID is public build configuration. Errors carry GitHub's
// error code or the HTTP status, never a response body, because a body can hold a token. Nothing
// here logs.

import Foundation

/// A pending authorization: the code the user enters on GitHub and the secret the app polls with.
struct DeviceAuthorization: Sendable, Equatable {
    var deviceCode: String
    var userCode: String
    var verificationURL: URL
    var expiresAt: Date
    /// Minimum seconds between token requests, as GitHub asked.
    var interval: TimeInterval
}

extension DeviceAuthorization: CustomStringConvertible, CustomDebugStringConvertible {
    var description: String { "DeviceAuthorization(userCode: \(userCode), expiresAt: \(expiresAt))" }
    var debugDescription: String { description }
}

enum GitHubAuthError: Error, Sendable, Hashable {
    /// The build has no OAuth client ID.
    case unavailable
    /// The code expired before the user authorized it.
    case expired
    /// The user canceled on GitHub.
    case denied
    /// The OAuth app does not have device flow enabled.
    case deviceFlowDisabled
    /// GitHub rejected the token when reading the account.
    case unauthorized
    case network(String)
    /// An error code or HTTP status the flow does not expect.
    case unexpected(String)
    case keychain(String)
}

extension GitHubAuthError: LocalizedError {
    var errorDescription: String? {
        switch self {
        case .unavailable:
            String(
                localized: """
                    This build of Ambit has no GitHub OAuth client ID, so it cannot sign in to GitHub. \
                    Public and local catalogs work without signing in.
                    """)
        case .expired:
            String(localized: "The code expired before it was entered on GitHub. Sign in again to get a new code.")
        case .denied:
            String(localized: "Authorization was canceled on GitHub.")
        case .deviceFlowDisabled:
            String(localized: "The Ambit GitHub app does not allow device sign-in. Ask the maintainers to enable it.")
        case .unauthorized:
            String(localized: "GitHub did not accept the sign-in. Sign in again.")
        case let .network(message):
            String(localized: "Could not reach GitHub. \(message)")
        case let .unexpected(code):
            String(localized: "GitHub returned an unexpected response (\(code)).")
        case let .keychain(message):
            String(localized: "Could not use the Keychain. \(message)")
        }
    }
}

struct DeviceFlowClient: Sendable {
    /// Private catalogs need read access to private repositories, which only `repo` grants.
    static let scope = "repo"
    /// GitHub adds this many seconds to the interval with every `slow_down`.
    static let slowDownIncrement: TimeInterval = 5

    static let deviceCodeURL = URL(string: "https://github.com/login/device/code")!
    static let tokenURL = URL(string: "https://github.com/login/oauth/access_token")!
    static let userURL = URL(string: "https://api.github.com/user")!

    var transport: any GitHubTransport
    var clock: any DeviceFlowClock = SystemDeviceFlowClock()

    func requestAuthorization(clientID: String) async throws -> DeviceAuthorization {
        let response: DeviceCodeResponse = try await post(
            Self.deviceCodeURL, form: ["client_id": clientID, "scope": Self.scope])

        if let error = response.error {
            throw Self.error(for: error)
        }
        guard let deviceCode = response.deviceCode, let userCode = response.userCode,
            let uri = response.verificationUri, let url = URL(string: uri), let expiresIn = response.expiresIn
        else {
            throw GitHubAuthError.unexpected("device code")
        }

        return DeviceAuthorization(
            deviceCode: deviceCode, userCode: userCode, verificationURL: url,
            expiresAt: clock.now.addingTimeInterval(expiresIn), interval: response.interval ?? 5)
    }

    /// Polls until the user authorizes, denies, or the code expires.
    ///
    /// Throws `CancellationError` when the calling task is cancelled, `GitHubAuthError.expired`,
    /// `.denied`, or another `GitHubAuthError`.
    func waitForToken(clientID: String, authorization: DeviceAuthorization) async throws -> GitHubToken {
        var interval = authorization.interval

        while true {
            try await clock.sleep(seconds: interval)
            try Task.checkCancellation()
            if clock.now >= authorization.expiresAt {
                throw GitHubAuthError.expired
            }

            let response: TokenResponse = try await post(
                Self.tokenURL,
                form: [
                    "client_id": clientID, "device_code": authorization.deviceCode,
                    "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
                ])
            try Task.checkCancellation()

            if let token = response.accessToken, !token.isEmpty {
                return GitHubToken(token)
            }

            switch response.error {
            case "authorization_pending":
                continue
            case "slow_down":
                interval = max(interval + Self.slowDownIncrement, response.interval ?? 0)
            case let code?:
                throw Self.error(for: code)
            case nil:
                throw GitHubAuthError.unexpected("token")
            }
        }
    }

    /// The login of the account that owns `token`.
    func username(token: GitHubToken) async throws -> String {
        var request = URLRequest(url: Self.userURL)
        request.setValue("Bearer \(token.value)", forHTTPHeaderField: "Authorization")
        request.setValue("application/vnd.github+json", forHTTPHeaderField: "Accept")
        request.setValue("2022-11-28", forHTTPHeaderField: "X-GitHub-Api-Version")
        request.setValue("Ambit", forHTTPHeaderField: "User-Agent")

        let (data, response) = try await send(request)
        if response.statusCode == 401 {
            throw GitHubAuthError.unauthorized
        }
        guard response.statusCode == 200 else {
            throw GitHubAuthError.unexpected("HTTP \(response.statusCode)")
        }
        guard let user = try? JSONDecoder().decode(UserResponse.self, from: data), !user.login.isEmpty else {
            throw GitHubAuthError.unexpected("user")
        }
        return user.login
    }

    private static func error(for code: String) -> GitHubAuthError {
        switch code {
        case "expired_token": .expired
        case "access_denied": .denied
        case "device_flow_disabled": .deviceFlowDisabled
        default: .unexpected(code)
        }
    }

    private func post<Response: Decodable>(_ url: URL, form: [String: String]) async throws -> Response {
        var request = URLRequest(url: url)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("application/x-www-form-urlencoded", forHTTPHeaderField: "Content-Type")
        request.setValue("Ambit", forHTTPHeaderField: "User-Agent")
        var components = URLComponents()
        components.queryItems = form.sorted { $0.key < $1.key }.map { URLQueryItem(name: $0.key, value: $0.value) }
        request.httpBody = Data((components.percentEncodedQuery ?? "").utf8)

        let (data, response) = try await send(request)
        // GitHub answers device-flow errors with 200 and an `error` field; other statuses carry
        // no field this flow uses.
        guard response.statusCode == 200 else {
            throw GitHubAuthError.unexpected("HTTP \(response.statusCode)")
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        guard let decoded = try? decoder.decode(Response.self, from: data) else {
            throw GitHubAuthError.unexpected("unreadable response")
        }
        return decoded
    }

    private func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        do {
            return try await transport.send(request)
        } catch let error as URLError where error.code == .cancelled {
            throw CancellationError()
        } catch let error as URLError {
            throw GitHubAuthError.network(error.localizedDescription)
        }
    }
}

private struct DeviceCodeResponse: Decodable {
    var deviceCode: String?
    var userCode: String?
    var verificationUri: String?
    var expiresIn: TimeInterval?
    var interval: TimeInterval?
    var error: String?
}

private struct TokenResponse: Decodable {
    var accessToken: String?
    var interval: TimeInterval?
    var error: String?
}

private struct UserResponse: Decodable {
    var login: String
}
