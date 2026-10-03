// Unapplied edits to one setup's config, kept in memory only. Nothing here touches disk: the
// draft text reaches the setup root only through a reviewed apply (see `ReviewModel`).
// Every edit goes through the engine's lossless editor, so the draft keeps the saved file's
// comments and formatting outside the edited nodes.
//
// Feature areas never hold a draft. They go through their `SetupModel`:
// - `stage(_ edits: [ConfigEdit]) throws` adds edits, starting the draft on first use.
// - `configSummary` is the config as the user sees it (draft or saved); `draftText` is the text
//   to pass to `loadCatalogs`/`browse`/`ruleMatches` (`nil` means the saved config).
// - `hasPendingChanges`, `discardChanges()` and `startReview()` drive the header actions.
// - Fetching or writing engine calls run through `operations.run(title, setup: id) { progress in
//   ... }`, which serializes them app-wide and publishes stage and progress in
//   `operations.current`.

import Foundation
import Observation

@MainActor
@Observable
final class DraftModel {
    /// The file name a new setup's config gets.
    nonisolated static let newFileName = "ambit.yml"

    /// The saved config the draft started from: `.missing` for a new setup, otherwise `.valid`.
    /// A snapshot that differs from it means the file changed outside the app.
    let base: ConfigState
    let fileName: String
    private(set) var edits: [ConfigEdit] = []
    private(set) var text: String
    private(set) var summary: ConfigSummary

    @ObservationIgnored private let engine: any EngineService

    /// A draft of a saved config. `base` must be `.valid`.
    init(editing base: ConfigState, engine: any EngineService) {
        guard case let .valid(_, fileName, text, summary) = base else {
            preconditionFailure("Only a valid config can be edited.")
        }

        self.base = base
        self.fileName = fileName
        self.engine = engine
        self.text = text
        self.summary = summary
    }

    /// A draft that creates a config with `harnesses`.
    ///
    /// Throws the engine's error when the new config does not parse.
    init(newWith harnesses: [String], engine: any EngineService) throws {
        base = .missing
        fileName = Self.newFileName
        self.engine = engine
        let text = engine.newConfigText(harnesses: harnesses)
        self.text = text
        summary = try engine.parseConfig(text: text, fileName: Self.newFileName)
    }

    var baseText: String? {
        if case let .valid(_, _, text, _) = base { text } else { nil }
    }

    var isNew: Bool { base == .missing }

    /// True when applying the draft would change the saved config. A new setup's draft is always
    /// dirty, even before any edit, because applying it creates the file.
    var isDirty: Bool { text != baseText }

    /// Applies `newEdits` on top of the earlier ones. On failure the draft is unchanged.
    func stage(_ newEdits: [ConfigEdit]) throws {
        let result = try engine.editConfig(text: text, fileName: fileName, edits: newEdits)
        edits += newEdits
        text = result.text
        summary = result.summary
    }

    /// Rebuilds a new setup's draft from a config with `harnesses`, replaying the edits made since.
    func replaceNewHarnesses(_ harnesses: [String]) throws {
        precondition(isNew, "Only a new setup's draft has a seed to replace.")

        let newSeed = engine.newConfigText(harnesses: harnesses)
        let result =
            if edits.isEmpty {
                EditedConfig(text: newSeed, summary: try engine.parseConfig(text: newSeed, fileName: fileName))
            } else {
                try engine.editConfig(text: newSeed, fileName: fileName, edits: edits)
            }
        text = result.text
        summary = result.summary
    }
}
