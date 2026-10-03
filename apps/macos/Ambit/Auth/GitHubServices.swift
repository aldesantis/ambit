import Foundation

/// What `GitHubAccountModel` needs from the outside, chosen once at launch.
struct GitHubServices: Sendable {
    static let clientIDInfoKey = "AmbitGitHubClientID"
    /// Used under UI testing when the build has no client ID, so the fake flow can run.
    static let testClientID = "ambit-ui-test-client"

    /// `nil` when the build has no OAuth client ID; sign-in is then unavailable.
    var clientID: String?
    var client: DeviceFlowClient
    var tokenStore: any GitHubTokenStore

    /// No client ID, no network, nothing persisted. The default for environments built by tests.
    static var inert: GitHubServices {
        GitHubServices(
            clientID: nil, client: DeviceFlowClient(transport: URLSessionGitHubTransport()),
            tokenStore: InMemoryTokenStore())
    }

    static func make(for launch: LaunchContext, bundle: Bundle = .main) -> GitHubServices {
        let bundled = clientID(in: bundle)

        if let hooks = launch.testHooks {
            let store: any GitHubTokenStore =
                hooks.inMemoryKeychain
                ? InMemoryTokenStore() : KeychainTokenStore(service: KeychainTokenStore.defaultService + ".uitest")
            guard let name = hooks.gitHubScenario else {
                return GitHubServices(
                    clientID: bundled, client: DeviceFlowClient(transport: URLSessionGitHubTransport()),
                    tokenStore: store)
            }
            // "unavailable" simulates a build without a client ID.
            let scenario = FakeGitHubTransport.Scenario(rawValue: name) ?? .success
            return GitHubServices(
                clientID: name == "unavailable" ? nil : bundled ?? testClientID,
                client: DeviceFlowClient(transport: FakeGitHubTransport(scenario: scenario)), tokenStore: store)
        }

        // The unit test host must not read or push the user's real token.
        if launch.isUnitTestHost {
            return .inert
        }

        return GitHubServices(
            clientID: bundled, client: DeviceFlowClient(transport: URLSessionGitHubTransport()),
            tokenStore: KeychainTokenStore())
    }

    /// The Info.plist value, or `nil` when it is empty or the build setting was not expanded.
    static func clientID(in bundle: Bundle) -> String? {
        let value = (bundle.object(forInfoDictionaryKey: clientIDInfoKey) as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard let value, !value.isEmpty, !value.hasPrefix("$(") else {
            return nil
        }
        return value
    }
}
