import Foundation
import Synchronization
import Testing

@testable import Ambit

/// Answers each request with the next scripted response for its URL and records every request.
final class ScriptedTransport: GitHubTransport {
    enum Reply: Sendable {
        case json(String, status: Int = 200)
        case failure(URLError.Code)
    }

    private let state: Mutex<(replies: [URL: [Reply]], requests: [URLRequest])>

    init(_ replies: [URL: [Reply]]) {
        state = Mutex((replies, []))
    }

    var requests: [URLRequest] { state.withLock { $0.requests } }

    func requests(to url: URL) -> [URLRequest] { requests.filter { $0.url == url } }

    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let reply = state.withLock { state -> Reply? in
            state.requests.append(request)
            guard var queue = state.replies[request.url!], !queue.isEmpty else {
                return nil
            }
            // The last reply repeats.
            let next = queue.count > 1 ? queue.removeFirst() : queue[0]
            state.replies[request.url!] = queue
            return next
        }

        switch reply {
        case let .json(body, status):
            let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!
            return (Data(body.utf8), response)
        case let .failure(code):
            throw URLError(code)
        case nil:
            Issue.record("Unexpected request to \(request.url!)")
            throw URLError(.unsupportedURL)
        }
    }
}

/// Virtual time: `sleep` advances `now` immediately and records the duration.
final class TestClock: DeviceFlowClock {
    private let state = Mutex((now: Date(timeIntervalSince1970: 1_000_000), sleeps: [TimeInterval]()))

    var now: Date { state.withLock { $0.now } }
    var sleeps: [TimeInterval] { state.withLock { $0.sleeps } }

    func sleep(seconds: TimeInterval) async throws {
        try Task.checkCancellation()
        state.withLock {
            $0.sleeps.append(seconds)
            $0.now.addTimeInterval(seconds)
        }
        await Task.yield()
        try Task.checkCancellation()
    }
}

/// A clock whose `sleep` suspends until the task is cancelled, for cancellation tests.
struct BlockingClock: DeviceFlowClock {
    var now: Date { Date(timeIntervalSince1970: 1_000_000) }

    func sleep(seconds: TimeInterval) async throws {
        while true {
            try Task.checkCancellation()
            try await Task.sleep(for: .milliseconds(10))
        }
    }
}

enum Fixtures {
    static let token = "gho_SECRET_token_value_123"

    static func deviceCode(expiresIn: Int = 900, interval: Int = 5) -> ScriptedTransport.Reply {
        .json(
            """
            {"device_code":"dev-123","user_code":"WDJB-MJHT","verification_uri":"https://github.com/login/device",\
            "expires_in":\(expiresIn),"interval":\(interval)}
            """)
    }

    static let pending = ScriptedTransport.Reply.json(#"{"error":"authorization_pending"}"#)
    static let granted = ScriptedTransport.Reply.json(#"{"access_token":"\#(token)","token_type":"bearer","scope":"repo"}"#)
    static let user = ScriptedTransport.Reply.json(#"{"login":"octocat","id":1}"#)
}

struct DeviceFlowClientTests {
    private func client(_ transport: ScriptedTransport, clock: any DeviceFlowClock = TestClock()) -> DeviceFlowClient {
        DeviceFlowClient(transport: transport, clock: clock)
    }

    private func body(_ request: URLRequest) -> [String: String] {
        let text = String(decoding: request.httpBody ?? Data(), as: UTF8.self)
        var components = URLComponents()
        components.percentEncodedQuery = text
        return Dictionary(uniqueKeysWithValues: (components.queryItems ?? []).map { ($0.name, $0.value ?? "") })
    }

    @Test func requestsACodeWithTheRepoScope() async throws {
        let transport = ScriptedTransport([DeviceFlowClient.deviceCodeURL: [Fixtures.deviceCode()]])
        let clock = TestClock()

        let authorization = try await client(transport, clock: clock).requestAuthorization(clientID: "client-1")

        #expect(authorization.userCode == "WDJB-MJHT")
        #expect(authorization.verificationURL == URL(string: "https://github.com/login/device"))
        #expect(authorization.interval == 5)
        #expect(authorization.expiresAt == clock.now.addingTimeInterval(900))
        let request = try #require(transport.requests.first)
        #expect(request.httpMethod == "POST")
        #expect(request.value(forHTTPHeaderField: "Accept") == "application/json")
        #expect(body(request) == ["client_id": "client-1", "scope": "repo"])
    }

    @Test func pollsAtTheIntervalWhilePending() async throws {
        let transport = ScriptedTransport([
            DeviceFlowClient.deviceCodeURL: [Fixtures.deviceCode(interval: 5)],
            DeviceFlowClient.tokenURL: [Fixtures.pending, Fixtures.pending, Fixtures.granted],
        ])
        let clock = TestClock()
        let client = client(transport, clock: clock)

        let authorization = try await client.requestAuthorization(clientID: "client-1")
        let token = try await client.waitForToken(clientID: "client-1", authorization: authorization)

        #expect(token.value == Fixtures.token)
        #expect(clock.sleeps == [5, 5, 5])
        let poll = try #require(transport.requests(to: DeviceFlowClient.tokenURL).first)
        #expect(
            body(poll) == [
                "client_id": "client-1", "device_code": "dev-123",
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            ])
    }

    @Test func slowDownAddsFiveSecondsOrUsesTheNewInterval() async throws {
        let transport = ScriptedTransport([
            DeviceFlowClient.deviceCodeURL: [Fixtures.deviceCode(interval: 5)],
            DeviceFlowClient.tokenURL: [
                .json(#"{"error":"slow_down"}"#), .json(#"{"error":"slow_down","interval":20}"#), Fixtures.pending,
                Fixtures.granted,
            ],
        ])
        let clock = TestClock()
        let client = client(transport, clock: clock)

        let authorization = try await client.requestAuthorization(clientID: "c")
        _ = try await client.waitForToken(clientID: "c", authorization: authorization)

        #expect(clock.sleeps == [5, 10, 20, 20])
    }

    @Test func expiredTokenErrorStopsPolling() async throws {
        let transport = ScriptedTransport([
            DeviceFlowClient.deviceCodeURL: [Fixtures.deviceCode()],
            DeviceFlowClient.tokenURL: [Fixtures.pending, .json(#"{"error":"expired_token"}"#)],
        ])
        let client = client(transport)

        let authorization = try await client.requestAuthorization(clientID: "c")
        await #expect(throws: GitHubAuthError.expired) {
            try await client.waitForToken(clientID: "c", authorization: authorization)
        }
        #expect(transport.requests(to: DeviceFlowClient.tokenURL).count == 2)
    }

    @Test func stopsAtTheExpiryWithoutAskingGitHub() async throws {
        let transport = ScriptedTransport([
            DeviceFlowClient.deviceCodeURL: [Fixtures.deviceCode(expiresIn: 12, interval: 5)],
            DeviceFlowClient.tokenURL: [Fixtures.pending],
        ])
        let client = client(transport)

        let authorization = try await client.requestAuthorization(clientID: "c")
        await #expect(throws: GitHubAuthError.expired) {
            try await client.waitForToken(clientID: "c", authorization: authorization)
        }
        // Polls at 5 and 10 seconds; the 15-second poll is past the expiry.
        #expect(transport.requests(to: DeviceFlowClient.tokenURL).count == 2)
    }

    @Test func accessDeniedIsDenied() async throws {
        let transport = ScriptedTransport([
            DeviceFlowClient.deviceCodeURL: [Fixtures.deviceCode()],
            DeviceFlowClient.tokenURL: [.json(#"{"error":"access_denied"}"#)],
        ])
        let client = client(transport)

        let authorization = try await client.requestAuthorization(clientID: "c")
        await #expect(throws: GitHubAuthError.denied) {
            try await client.waitForToken(clientID: "c", authorization: authorization)
        }
    }

    @Test func deviceFlowDisabledIsReported() async {
        let transport = ScriptedTransport([
            DeviceFlowClient.deviceCodeURL: [.json(#"{"error":"device_flow_disabled"}"#)]
        ])

        await #expect(throws: GitHubAuthError.deviceFlowDisabled) {
            try await client(transport).requestAuthorization(clientID: "c")
        }
    }

    @Test func cancellationStopsPolling() async throws {
        let transport = ScriptedTransport([
            DeviceFlowClient.deviceCodeURL: [Fixtures.deviceCode()],
            DeviceFlowClient.tokenURL: [Fixtures.pending],
        ])
        let client = client(transport, clock: BlockingClock())
        let authorization = try await client.requestAuthorization(clientID: "c")

        let task = Task { try await client.waitForToken(clientID: "c", authorization: authorization) }
        try await Task.sleep(for: .milliseconds(50))
        task.cancel()

        await #expect(throws: CancellationError.self) { try await task.value }
        #expect(transport.requests(to: DeviceFlowClient.tokenURL).isEmpty)
    }

    @Test func networkFailuresAreNetworkErrors() async {
        let transport = ScriptedTransport([DeviceFlowClient.deviceCodeURL: [.failure(.notConnectedToInternet)]])

        await #expect {
            try await client(transport).requestAuthorization(clientID: "c")
        } throws: { error in
            if case .network = error as? GitHubAuthError { true } else { false }
        }
    }

    @Test func readsTheUsernameWithTheToken() async throws {
        let transport = ScriptedTransport([DeviceFlowClient.userURL: [Fixtures.user]])

        let username = try await client(transport).username(token: GitHubToken(Fixtures.token))

        #expect(username == "octocat")
        let request = try #require(transport.requests.first)
        #expect(request.value(forHTTPHeaderField: "Authorization") == "Bearer \(Fixtures.token)")
    }

    @Test func rejectedTokenIsUnauthorized() async {
        let transport = ScriptedTransport([
            DeviceFlowClient.userURL: [.json(#"{"message":"Bad credentials"}"#, status: 401)]
        ])

        await #expect(throws: GitHubAuthError.unauthorized) {
            try await client(transport).username(token: GitHubToken(Fixtures.token))
        }
    }
}

@MainActor
struct GitHubAccountModelTests {
    private let engine = FakeEngineService()
    private let store = InMemoryTokenStore()

    private func model(_ transport: ScriptedTransport, clientID: String? = "client-1", clock: any DeviceFlowClock = TestClock())
        -> GitHubAccountModel
    {
        GitHubAccountModel(
            services: GitHubServices(
                clientID: clientID, client: DeviceFlowClient(transport: transport, clock: clock), tokenStore: store),
            engine: engine)
    }

    private static func signInScript(token: [ScriptedTransport.Reply] = [Fixtures.pending, Fixtures.granted])
        -> ScriptedTransport
    {
        ScriptedTransport([
            DeviceFlowClient.deviceCodeURL: [Fixtures.deviceCode()],
            DeviceFlowClient.tokenURL: token,
            DeviceFlowClient.userURL: [Fixtures.user],
        ])
    }

    @Test func signInStoresTheAccountAndPassesTheTokenToTheEngine() async throws {
        let model = model(Self.signInScript())

        model.signIn()
        #expect(model.state == .requestingCode)
        await model.flow?.value

        #expect(model.state == .signedIn(username: "octocat"))
        #expect(try store.load() == GitHubAccount(username: "octocat", token: GitHubToken(Fixtures.token)))
        #expect(engine.gitHubToken == Fixtures.token)
    }

    @Test func showsTheCodeWhileWaiting() async throws {
        let model = model(Self.signInScript(token: [Fixtures.pending]), clock: BlockingClock())

        model.signIn()
        while model.state == .requestingCode {
            await Task.yield()
        }

        guard case let .awaitingUser(code, url, _) = model.state else {
            Issue.record("Expected awaitingUser, got \(model.state)")
            return
        }
        #expect(code == "WDJB-MJHT")
        #expect(url.absoluteString == "https://github.com/login/device")

        model.cancelSignIn()
        #expect(model.state == .signedOut)
        #expect(engine.gitHubToken == nil)
        #expect(try store.load() == nil)
    }

    @Test func cancelingASecondSignInKeepsTheAccount() async throws {
        try store.save(GitHubAccount(username: "hubot", token: GitHubToken("old")))
        let model = model(Self.signInScript(token: [Fixtures.pending]), clock: BlockingClock())
        await model.restore()

        model.signIn()
        model.cancelSignIn()

        #expect(model.state == .signedIn(username: "hubot"))
        #expect(engine.gitHubToken == "old")
    }

    @Test(arguments: [
        ("access_denied", GitHubAuthError.denied), ("expired_token", GitHubAuthError.expired),
    ])
    func failedAuthorizationShowsAnErrorAndStoresNothing(code: String, expected: GitHubAuthError) async throws {
        let model = model(Self.signInScript(token: [.json(#"{"error":"\#(code)"}"#)]))

        model.signIn()
        await model.flow?.value

        #expect(model.state == .error(expected))
        #expect(try store.load() == nil)
        #expect(engine.gitHubToken == nil)

        model.dismissError()
        #expect(model.state == .signedOut)
    }

    @Test func withoutAClientIDSignInIsUnavailable() {
        let model = model(ScriptedTransport([:]), clientID: nil)

        #expect(!model.isAvailable)
        model.signIn()
        #expect(model.state == .error(.unavailable))
    }

    @Test func restorePassesTheStoredTokenWithoutNetwork() async throws {
        try store.save(GitHubAccount(username: "octocat", token: GitHubToken(Fixtures.token)))
        let transport = ScriptedTransport([:])
        let model = model(transport)

        await model.restore()

        #expect(model.state == .signedIn(username: "octocat"))
        #expect(engine.gitHubToken == Fixtures.token)
        #expect(transport.requests.isEmpty)
    }

    @Test func signOutDeletesTheAccountAndClearsTheEngine() async throws {
        try store.save(GitHubAccount(username: "octocat", token: GitHubToken(Fixtures.token)))
        let model = model(ScriptedTransport([:]))
        await model.restore()

        await model.signOut()

        #expect(model.state == .signedOut)
        #expect(try store.load() == nil)
        #expect(engine.gitHubToken == nil)
    }
}

/// Uses the real Keychain under a throwaway service name and removes it afterwards.
@Suite(.serialized)
struct KeychainTokenStoreTests {
    private let store = KeychainTokenStore(service: "com.nebulab.ambit.github.tests.\(UUID().uuidString)")

    @Test func savesLoadsReplacesAndDeletes() throws {
        defer { try? store.delete() }

        #expect(try store.load() == nil)

        let first = GitHubAccount(username: "octocat", token: GitHubToken("gho_first"))
        try store.save(first)
        #expect(try store.load() == first)

        let second = GitHubAccount(username: "hubot", token: GitHubToken("gho_second"))
        try store.save(second)
        #expect(try store.load() == second)

        try store.delete()
        #expect(try store.load() == nil)
        try store.delete()
    }
}

struct RedactionTests {
    @Test func tokenNeverPrints() {
        let token = GitHubToken(Fixtures.token)
        let account = GitHubAccount(username: "octocat", token: token)

        var dumped = ""
        dump(account, to: &dumped)
        let renderings = [
            String(describing: token), String(reflecting: token), "\(token)", String(describing: account),
            String(reflecting: account), dumped,
        ]

        for rendering in renderings {
            #expect(!rendering.contains(Fixtures.token), "\(rendering)")
        }
    }

    @Test func errorsNeverCarryTheResponseBody() async {
        // A misbehaving server echoing the token in an error body.
        let transport = ScriptedTransport([
            DeviceFlowClient.userURL: [.json(#"{"token":"\#(Fixtures.token)"}"#, status: 500)]
        ])
        let client = DeviceFlowClient(transport: transport, clock: TestClock())

        do {
            _ = try await client.username(token: GitHubToken(Fixtures.token))
            Issue.record("Expected an error")
        } catch {
            #expect(!String(describing: error).contains(Fixtures.token))
            #expect(!error.localizedDescription.contains(Fixtures.token))
        }
    }

    @MainActor
    @Test func modelStateNeverHoldsTheToken() async {
        let transport = ScriptedTransport([
            DeviceFlowClient.deviceCodeURL: [Fixtures.deviceCode()],
            DeviceFlowClient.tokenURL: [Fixtures.granted],
            DeviceFlowClient.userURL: [Fixtures.user],
        ])
        let model = GitHubAccountModel(
            services: GitHubServices(
                clientID: "c", client: DeviceFlowClient(transport: transport, clock: TestClock()),
                tokenStore: InMemoryTokenStore()),
            engine: FakeEngineService())

        model.signIn()
        await model.flow?.value

        var dumped = ""
        dump(model.state, to: &dumped)
        #expect(!dumped.contains(Fixtures.token))
        #expect(!String(describing: model.state).contains(Fixtures.token))
    }
}

struct GitHubAccessExplanationTests {
    @Test func explainsSSOAndOffersSignIn() throws {
        let url = URL(string: "https://github.com/settings/connections/applications/c")!
        let explanation = try #require(
            GitHubAccessExplanation(kind: .accessDenied(sso: true), signedInAs: "octocat", authorizationURL: url))

        #expect(explanation.message.contains("single sign-on"))
        #expect(explanation.offersSignIn)
        #expect(explanation.authorizationURL == url)
    }

    @Test func ignoresFailuresThatAreNotAboutAccess() {
        #expect(GitHubAccessExplanation(kind: .offline, signedInAs: nil) == nil)
    }
}

struct GitHubServicesTests {
    @Test func unitTestHostNeverUsesTheRealKeychain() {
        let services = GitHubServices.make(for: LaunchContext(isUnitTestHost: true))

        #expect(services.tokenStore is InMemoryTokenStore)
        #expect(services.clientID == nil)
    }

    @Test func uiTestScenarioUsesTheFakeTransport() {
        var hooks = LaunchContext.TestHooks()
        hooks.gitHubScenario = "success"
        hooks.inMemoryKeychain = true

        let services = GitHubServices.make(for: LaunchContext(testHooks: hooks))

        #expect(services.client.transport is FakeGitHubTransport)
        #expect(services.tokenStore is InMemoryTokenStore)
        #expect(services.clientID != nil)
    }
}
