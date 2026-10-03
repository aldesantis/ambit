// Scripted GitHub responses for UI tests (`AMBIT_TEST_GITHUB=<scenario>`). It answers the three
// endpoints the device flow calls and never touches the network.

import Foundation
import Synchronization

final class FakeGitHubTransport: GitHubTransport {
    enum Scenario: String, Sendable {
        /// One pending poll, then a token.
        case success
        case denied
        case expired
        /// One `slow_down`, then a token.
        case slowDown = "slow_down"
    }

    static let userCode = "AMBT-1234"
    static let username = "octocat"
    static let token = "gho_fake_ui_test_token"

    let scenario: Scenario
    /// Seconds GitHub asks the app to wait between polls. Kept short so UI tests stay fast.
    let interval: Int
    private let polls = Mutex(0)

    init(scenario: Scenario, interval: Int = 1) {
        self.scenario = scenario
        self.interval = interval
    }

    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let json: String
        switch request.url {
        case DeviceFlowClient.deviceCodeURL:
            json = """
                {"device_code":"fake-device-code","user_code":"\(Self.userCode)",\
                "verification_uri":"https://github.com/login/device","expires_in":900,"interval":\(interval)}
                """
        case DeviceFlowClient.tokenURL:
            json = tokenResponse(poll: polls.withLock { count in
                count += 1
                return count
            })
        case DeviceFlowClient.userURL:
            json = #"{"login":"\#(Self.username)"}"#
        default:
            return (Data(), Self.response(for: request, status: 404))
        }
        return (Data(json.utf8), Self.response(for: request, status: 200))
    }

    private func tokenResponse(poll: Int) -> String {
        switch (scenario, poll) {
        case (.denied, _): #"{"error":"access_denied"}"#
        case (.expired, _): #"{"error":"expired_token"}"#
        case (.success, 1): #"{"error":"authorization_pending"}"#
        case (.slowDown, 1): #"{"error":"slow_down","interval":\#(interval)}"#
        case (.success, _), (.slowDown, _): #"{"access_token":"\#(Self.token)","token_type":"bearer","scope":"repo"}"#
        }
    }

    private static func response(for request: URLRequest, status: Int) -> HTTPURLResponse {
        HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: "HTTP/1.1", headerFields: nil)!
    }
}
