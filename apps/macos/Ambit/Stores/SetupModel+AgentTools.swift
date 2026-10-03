import Foundation

extension SetupModel {
    /// The Agent tools area of this setup, created on first use and kept for the setup's lifetime.
    var agentTools: AgentToolsModel {
        SetupAreaModels.model(for: self, area: "agentTools") { AgentToolsModel(setup: $0) }
    }
}

/// Area models attached to a `SetupModel` from its extension files, which cannot add stored
/// properties. Entries hold the setup weakly and are dropped once it is gone, so a forgotten
/// project's area state never resurfaces on a new setup that reuses its memory address.
@MainActor
enum SetupAreaModels {
    private struct Entry {
        weak var setup: SetupModel?
        var models: [String: AnyObject]
    }

    private static var entries: [ObjectIdentifier: Entry] = [:]

    static func model<Model: AnyObject>(
        for setup: SetupModel, area: String, make: (SetupModel) -> Model
    ) -> Model {
        let key = ObjectIdentifier(setup)
        if entries[key]?.setup !== setup {
            entries = entries.filter { $0.value.setup != nil }
            entries[key] = Entry(setup: setup, models: [:])
        }

        if let model = entries[key]?.models[area] as? Model {
            return model
        }

        let model = make(setup)
        entries[key]?.models[area] = model
        return model
    }
}
