import AmbitEngine
import Foundation
import Testing

/// Proves the bindings link and round-trip through the Rust library.
struct EngineTests {
    private func temporaryRoot() throws -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("ambit-engine-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    private func engine() -> Engine {
        Engine(config: EngineConfig(env: ["HOME": "/nonexistent"]))
    }

    @Test func snapshotOfAnEmptyRootIsMissing() throws {
        let root = try temporaryRoot()
        defer { try? FileManager.default.removeItem(at: root) }

        let snapshot = try engine().openSetup(root: root.path).snapshot()

        #expect(snapshot.root == root.path)
        #expect(snapshot.config == .missing)
    }

    @Test func snapshotReadsAValidConfig() throws {
        let root = try temporaryRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        let text = "version: 1\nharnesses: [cursor]\n"
        try text.write(to: root.appendingPathComponent("ambit.yml"), atomically: true, encoding: .utf8)

        let snapshot = try engine().openSetup(root: root.path).snapshot()

        guard case let .valid(_, fileName, readText, summary) = snapshot.config else {
            Issue.record("expected a valid config, got \(snapshot.config)")
            return
        }
        #expect(fileName == "ambit.yml")
        #expect(readText == text)
        #expect(summary.harnesses == ["cursor"])
    }

    @Test func invalidConfigCarriesTheLine() throws {
        let root = try temporaryRoot()
        defer { try? FileManager.default.removeItem(at: root) }
        try "version: 1\nextra: true\n".write(
            to: root.appendingPathComponent("ambit.yml"), atomically: true, encoding: .utf8)

        let snapshot = try engine().openSetup(root: root.path).snapshot()

        guard case let .invalid(_, _, problem) = snapshot.config else {
            Issue.record("expected an invalid config, got \(snapshot.config)")
            return
        }
        #expect(problem.line == 2)
    }

    @Test func errorsAreThrownAsEngineError() {
        #expect(throws: EngineError.self) {
            _ = try describeSource(source: "not a source", gitRef: nil)
        }
    }

    @Test func listsTheFiveAgentTools() {
        #expect(supportedAgentTools().map(\.id) == ["claude", "codex", "cursor", "opencode", "vscode"])
    }
}
