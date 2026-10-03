import Foundation

/// Sends one HTTP request to GitHub. The live transport is `URLSession`; tests and the UI-test
/// hook substitute scripted responses.
protocol GitHubTransport: Sendable {
    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse)
}

struct URLSessionGitHubTransport: GitHubTransport {
    var session: URLSession = .shared

    func send(_ request: URLRequest) async throws -> (Data, HTTPURLResponse) {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw URLError(.badServerResponse)
        }
        return (data, http)
    }
}

/// Time as the device flow sees it, so tests can run the polling loop without waiting.
protocol DeviceFlowClock: Sendable {
    var now: Date { get }

    /// Throws `CancellationError` when the calling task is cancelled.
    func sleep(seconds: TimeInterval) async throws
}

struct SystemDeviceFlowClock: DeviceFlowClock {
    var now: Date { Date() }

    func sleep(seconds: TimeInterval) async throws {
        try await Task.sleep(for: .seconds(seconds))
    }
}
