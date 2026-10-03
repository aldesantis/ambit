// The Agent tools area: which of the engine's adapters a setup installs for. Toggling a tool
// stages a `setHarnesses` edit in the setup's draft; nothing reaches disk before a reviewed apply.
// A setup keeps at least one tool, so the last one cannot be turned off.
//
// The app never computes how a tool combines the Personal setup with a project's files. Each tool
// reads both on its own terms, so this model only reports which files each setup writes.

import Foundation
import Observation

@MainActor
@Observable
final class AgentToolsModel {
    /// How a tool's selection differs between the draft and the saved config.
    enum Change: Equatable {
        case unchanged
        case added
        case removed
    }

    /// One file or folder a tool reads from this setup.
    struct WrittenFile: Equatable, Identifiable {
        var purpose: String
        /// Shown to the user: `~/`-prefixed in the Personal setup, project-relative otherwise.
        var path: String

        var id: String { purpose + path }
    }

    /// Why the last toggle did not stage, shown until the next successful toggle.
    private(set) var refusal: String?
    /// An edit the engine refused.
    private(set) var editError: EngineError?

    let tools: [AgentToolInfo]

    @ObservationIgnored private weak var setup: SetupModel?

    init(setup: SetupModel) {
        self.setup = setup
        tools = setup.engine.supportedAgentTools()
    }

    /// The tools of the config as the user sees it (draft or saved), in config order. Includes
    /// names this app version does not know, which are kept as they are.
    var selected: [String] { setup?.configSummary?.harnesses ?? [] }

    /// The tools of the saved config, or none when the setup has no valid config.
    var saved: [String] {
        if case let .valid(_, _, _, summary) = setup?.snapshot?.config {
            return summary.harnesses
        }
        return []
    }

    /// False when there is no config to edit yet (the new-setup flow chooses the first tools).
    var canEdit: Bool { setup?.configSummary != nil }

    /// Harness names in the config that none of the adapters handle.
    var unknownSelected: [String] {
        selected.filter { name in !tools.contains { $0.id == name } }
    }

    func isSelected(_ id: String) -> Bool { selected.contains(id) }

    /// True when `id` is the only tool left, so turning it off is refused.
    func isLastSelected(_ id: String) -> Bool { selected == [id] }

    func change(for id: String) -> Change {
        switch (saved.contains(id), selected.contains(id)) {
        case (false, true): .added
        case (true, false): .removed
        default: .unchanged
        }
    }

    /// Stages turning `id` on or off. Returns false, with `refusal` or `editError` set, when the
    /// change was not staged; the draft is then unchanged.
    @discardableResult
    func setSelected(_ id: String, _ on: Bool) -> Bool {
        guard let setup else {
            return false
        }

        let current = selected
        if on == current.contains(id) {
            return true
        }

        var next = current.filter { $0 != id }
        if on {
            next = ordered(next + [id])
        } else if next.isEmpty {
            refusal = String(
                localized:
                    "A setup needs at least one agent tool. Turn on another tool before turning off \(displayName(id)).")
            return false
        }

        do {
            try setup.stage([.setHarnesses(harnesses: next)])
            refusal = nil
            editError = nil
            return true
        } catch {
            editError = EngineError(error)
            return false
        }
    }

    func dismissMessages() {
        refusal = nil
        editError = nil
    }

    func displayName(_ id: String) -> String {
        tools.first { $0.id == id }?.displayName ?? id
    }

    /// The files and folders `tool` uses in this setup. MCP servers go to a different file in the
    /// Personal setup for some tools, so the location depends on the setup.
    func writtenFiles(for tool: AgentToolInfo) -> [WrittenFile] {
        let personal = setup?.id == .personal
        let prefix = personal ? "~/" : ""
        var files = [
            WrittenFile(purpose: String(localized: "Skills"), path: prefix + tool.skillsDir),
            WrittenFile(
                purpose: String(localized: "MCP servers"), path: prefix + (personal ? tool.personalMcpFile : tool.mcpFile)),
        ]
        if let hooks = tool.hooksFile {
            files.append(WrittenFile(purpose: String(localized: "Hooks"), path: prefix + hooks))
        }
        return files
    }

    /// How this setup relates to the same tool's other scope. The tool decides how they combine;
    /// the app states that instead of predicting a merged result.
    func scopeNote(for tool: AgentToolInfo) -> String {
        if setup?.id == .personal {
            String(
                localized:
                    "These files apply wherever you use \(tool.displayName). When you also use it in a project with its own setup, \(tool.displayName) decides how your Personal setup and the project's files combine.")
        } else {
            String(
                localized:
                    "These files apply when \(tool.displayName) works in this project. \(tool.displayName) also reads your Personal setup and decides how the two combine; Ambit does not merge them.")
        }
    }

    /// `names` in adapter order, with unknown names kept after the known ones.
    private func ordered(_ names: [String]) -> [String] {
        let known = tools.map(\.id).filter(names.contains)
        return known + names.filter { !known.contains($0) }
    }
}
