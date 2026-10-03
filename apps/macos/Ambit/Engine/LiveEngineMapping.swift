// Conversions from the UniFFI records (`AmbitEngine.X`) to the facade's value types, which mirror
// them field for field. FFI names are always qualified: the app declares types with the same names.

import AmbitEngine
import Foundation

enum LiveEngineMapping {
    /// Runs `body`, rethrowing FFI errors as the app's `EngineError`. Anything else the bindings
    /// throw (a Rust panic caught by UniFFI, a lifting failure) becomes `internal`.
    static func translatingErrors<T>(_ body: () throws -> T) throws -> T {
        do {
            return try body()
        } catch let error as AmbitEngine.EngineError {
            throw engineError(error)
        } catch let error as EngineError {
            throw error
        } catch {
            throw EngineError.internal(message: String(localized: "The engine failed unexpectedly."), detail: ["\(error)"])
        }
    }

    static func engineError(_ error: AmbitEngine.EngineError) -> EngineError {
        switch error {
        case let .Config(message, detail, path, line):
            .config(message: message, detail: detail, path: path, line: line)
        case let .Resolution(message, detail):
            .resolution(message: message, detail: detail)
        case let .Network(message, detail, kind):
            .network(message: message, detail: detail, kind: networkKind(kind))
        case let .OwnershipConflict(message, detail, path):
            .ownershipConflict(message: message, detail: detail, path: path)
        case let .StaleReview(message, detail):
            .staleReview(message: message, detail: detail)
        case let .Busy(message, detail):
            .busy(message: message, detail: detail)
        case .Canceled:
            .canceled
        case let .Io(message, detail, path):
            .io(message: message, detail: detail, path: path)
        case let .Internal(message, detail):
            .internal(message: message, detail: detail)
        }
    }

    static func networkKind(_ kind: AmbitEngine.NetworkKind) -> NetworkKind {
        switch kind {
        case .notCached: .notCached
        case .offline: .offline
        case .authRequired: .authRequired
        case let .accessDenied(sso): .accessDenied(sso: sso)
        case .notFound: .notFound
        case .other: .other
        }
    }

    static func stage(_ stage: AmbitEngine.Stage) -> Stage {
        switch stage {
        case .loadingCatalogs: .loadingCatalogs
        case .fetching: .fetching
        case .resolving: .resolving
        case .planning: .planning
        case .checkingOwnership: .checkingOwnership
        case .savingConfig: .savingConfig
        case .writingFiles: .writingFiles
        case .removingFiles: .removingFiles
        case .writingRecords: .writingRecords
        }
    }

    static func progressEvent(_ event: AmbitEngine.ProgressEvent) -> ProgressEvent {
        ProgressEvent(stage: stage(event.stage), subject: event.subject, current: event.current, total: event.total)
    }

    static func itemKind(_ kind: AmbitEngine.ItemKind) -> ItemKind {
        switch kind {
        case .skill: .skill
        case .pack: .pack
        case .mcp: .mcp
        case .hook: .hook
        }
    }

    static func itemKind(_ kind: ItemKind) -> AmbitEngine.ItemKind {
        switch kind {
        case .skill: .skill
        case .pack: .pack
        case .mcp: .mcp
        case .hook: .hook
        }
    }

    /// A `requires` entry as the engine addresses it: `catalog/pattern`, or the bare pattern.
    static func entryAddress(_ entry: SelectionEntry) -> AmbitEngine.EntryAddress {
        let address = entry.catalog.map { "\($0)/\(entry.pattern)" } ?? entry.pattern
        return AmbitEngine.EntryAddress(kind: itemKind(entry.kind), address: address)
    }

    static func configEdit(_ edit: ConfigEdit) -> AmbitEngine.ConfigEdit {
        switch edit {
        case let .setHarnesses(harnesses):
            .setHarnesses(harnesses: harnesses)
        case let .addCatalog(name, source, gitRef):
            .addCatalog(name: name, source: source, gitRef: gitRef)
        case let .setCatalogSource(name, source, gitRef):
            .setCatalogSource(name: name, source: source, gitRef: gitRef)
        case let .renameCatalog(from, to):
            .renameCatalog(from: from, to: to)
        case let .removeCatalog(name):
            .removeCatalog(name: name)
        case let .addEntry(entry):
            .addEntry(entry: entryAddress(entry))
        case let .removeEntry(entry):
            .removeEntry(entry: entryAddress(entry))
        case let .replaceEntry(old, new):
            .replaceEntry(old: entryAddress(old), new: entryAddress(new))
        }
    }

    static func editedConfig(_ edited: AmbitEngine.EditedConfig) -> EditedConfig {
        EditedConfig(text: edited.text, summary: configSummary(edited.summary))
    }

    static func configChanges(_ changes: AmbitEngine.ConfigChanges) -> ConfigChanges {
        ConfigChanges(
            harnessesAdded: changes.harnessesAdded, harnessesRemoved: changes.harnessesRemoved,
            catalogsAdded: changes.catalogsAdded.map(catalogEntry),
            catalogsRemoved: changes.catalogsRemoved.map(catalogEntry),
            catalogsChanged: changes.catalogsChanged.map {
                CatalogChange(old: catalogEntry($0.before), new: catalogEntry($0.after))
            },
            catalogsRenamed: changes.catalogsRenamed.map { CatalogRename(from: $0.from, to: $0.to) },
            entriesAdded: changes.entriesAdded.map(selectionEntry),
            entriesRemoved: changes.entriesRemoved.map(selectionEntry))
    }

    static func sourceKind(_ kind: AmbitEngine.SourceKind) -> SourceKind {
        switch kind {
        case let .local(path): .local(path: path)
        case let .git(url, github, commitRef): .git(url: url, github: github, commitRef: commitRef)
        }
    }

    static func catalogEntry(_ entry: AmbitEngine.CatalogEntry) -> CatalogEntry {
        CatalogEntry(
            name: entry.name, source: entry.source, gitRef: entry.gitRef, sourceKind: entry.sourceKind.map(sourceKind))
    }

    static func selectionEntry(_ entry: AmbitEngine.SelectionEntry) -> SelectionEntry {
        SelectionEntry(kind: itemKind(entry.kind), catalog: entry.catalog, pattern: entry.pattern, isRule: entry.isRule)
    }

    static func configSummary(_ summary: AmbitEngine.ConfigSummary) -> ConfigSummary {
        ConfigSummary(
            harnesses: summary.harnesses, catalogs: summary.catalogs.map(catalogEntry),
            requires: summary.requires.map(selectionEntry))
    }

    static func configProblem(_ problem: AmbitEngine.ConfigProblem) -> ConfigProblem {
        ConfigProblem(message: problem.message, detail: problem.detail, line: problem.line)
    }

    static func configState(_ state: AmbitEngine.ConfigState) -> ConfigState {
        switch state {
        case .missing:
            .missing
        case let .ambiguous(files, problem):
            .ambiguous(files: files, problem: configProblem(problem))
        case let .invalid(path, fileName, problem):
            .invalid(path: path, fileName: fileName, problem: configProblem(problem))
        case let .valid(path, fileName, text, summary):
            .valid(path: path, fileName: fileName, text: text, summary: configSummary(summary))
        }
    }

    static func snapshot(_ snapshot: AmbitEngine.SetupSnapshot) -> SetupSnapshot {
        SetupSnapshot(root: snapshot.root, config: configState(snapshot.config))
    }

    static func sourceInfo(_ info: AmbitEngine.SourceInfo) -> SourceInfo {
        SourceInfo(kind: sourceKind(info.kind), proposedName: info.proposedName)
    }

    static func agentTool(_ tool: AmbitEngine.AgentToolInfo) -> AgentToolInfo {
        AgentToolInfo(
            id: tool.id, displayName: tool.displayName, skillsDir: tool.skillsDir, mcpFile: tool.mcpFile,
            personalMcpFile: tool.personalMcpFile, hooksFile: tool.hooksFile, limitations: tool.limitations)
    }
}
