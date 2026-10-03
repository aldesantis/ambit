import Foundation
import Testing

@testable import Ambit

@MainActor
struct AgentToolsModelTests {
    private let root = "/tmp/ambit-agent-tools-tests"

    private func setup(harnesses: [String]?, id: SetupID? = nil) async throws -> (SetupModel, FakeEngineService) {
        let engine = FakeEngineService()
        if let harnesses {
            let text = engine.newConfigText(harnesses: harnesses)
            engine.setFixture(.init(config: try FakeEngineService.validConfig(text, root: root)), root: root)
        }
        let setup = SetupModel(
            id: id ?? .project(path: root), root: URL(fileURLWithPath: root), engine: engine,
            operations: OperationRunner())
        await setup.refresh()
        return (setup, engine)
    }

    @Test func listsAllFiveAdapters() async throws {
        let (setup, _) = try await setup(harnesses: ["claude"])

        #expect(setup.agentTools.tools.map(\.id) == ["claude", "codex", "cursor", "opencode", "vscode"])
        #expect(setup.agentTools === setup.agentTools)
    }

    @Test func turningOnAToolStagesItInAdapterOrder() async throws {
        let (setup, _) = try await setup(harnesses: ["vscode"])
        let model = setup.agentTools

        #expect(model.setSelected("codex", true))

        #expect(setup.hasPendingChanges)
        #expect(setup.configSummary?.harnesses == ["codex", "vscode"])
        #expect(model.change(for: "codex") == .added)
        #expect(model.change(for: "vscode") == .unchanged)
    }

    @Test func theLastToolCannotBeTurnedOff() async throws {
        let (setup, _) = try await setup(harnesses: ["claude"])
        let model = setup.agentTools

        #expect(model.isLastSelected("claude"))
        #expect(!model.setSelected("claude", false))

        #expect(model.refusal?.contains("at least one agent tool") == true)
        #expect(!setup.hasPendingChanges)
        #expect(model.isSelected("claude"))
    }

    @Test func aToolCanBeSwappedByTurningOnAnotherFirst() async throws {
        let (setup, _) = try await setup(harnesses: ["claude"])
        let model = setup.agentTools

        #expect(model.setSelected("cursor", true))
        #expect(model.setSelected("claude", false))

        #expect(setup.configSummary?.harnesses == ["cursor"])
        #expect(model.change(for: "claude") == .removed)
        #expect(model.refusal == nil)
    }

    @Test func turningBackOnRemovesThePendingChange() async throws {
        let (setup, _) = try await setup(harnesses: ["claude"])
        let model = setup.agentTools

        model.setSelected("opencode", true)
        model.setSelected("opencode", false)

        #expect(!setup.hasPendingChanges)
    }

    @Test func aSetupWithoutConfigCannotBeEdited() async throws {
        let (setup, _) = try await setup(harnesses: nil)

        #expect(!setup.agentTools.canEdit)
        #expect(!setup.agentTools.setSelected("claude", true))
    }

    @Test func filesDependOnTheScope() async throws {
        let (project, _) = try await setup(harnesses: ["claude"])
        let (personal, _) = try await setup(harnesses: ["claude"], id: .personal)
        let claude = try #require(project.agentTools.tools.first { $0.id == "claude" })

        #expect(project.agentTools.writtenFiles(for: claude).map(\.path).contains(".mcp.json"))
        #expect(personal.agentTools.writtenFiles(for: claude).map(\.path).contains("~/.claude.json"))
        #expect(project.agentTools.scopeNote(for: claude).contains("decides how the two combine"))
    }
}
