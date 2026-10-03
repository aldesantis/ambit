// Conversions for the browse exports (crates/ambit-ffi/src/browse.rs). The facade's records are
// narrower than the FFI's in a few places, noted where they drop something: the facade keeps
// item chains as plain items, MCP environment and header names without values, and frontmatter
// as top-level fields.

import AmbitEngine
import Foundation

extension LiveEngineMapping {
    static func fetchPolicy(_ policy: FetchPolicy) -> AmbitEngine.FetchPolicy {
        switch policy {
        case .cacheOnly: .cacheOnly
        case .fetchMissing: .fetchMissing
        }
    }

    static func itemRef(_ item: AmbitEngine.ItemRef) -> ItemRef {
        ItemRef(kind: itemKind(item.kind), catalog: item.catalog, name: item.name)
    }

    static func itemRef(_ item: ItemRef) -> AmbitEngine.ItemRef {
        AmbitEngine.ItemRef(kind: itemKind(item.kind), catalog: item.catalog, name: item.name)
    }

    static func catalogLoadState(_ availability: AmbitEngine.CatalogAvailability) -> CatalogLoadState {
        let state: CatalogAvailability =
            switch availability.state {
            case let .loaded(commit, local): .available(commit: commit, local: local)
            // The error only says the cache lacks the catalog, which `notCached` already does.
            case .notCached: .notCached
            case let .failed(error): .failed(error: engineError(error))
            }
        return CatalogLoadState(name: availability.name, availability: state)
    }

    static func catalogsState(_ state: AmbitEngine.CatalogsState) -> CatalogsState {
        CatalogsState(catalogs: state.catalogs.map(catalogLoadState))
    }

    /// The FFI chain runs from the entry's root item to the item itself; the facade's runs the
    /// other way, from the requiring item up to the root, without the item.
    static func chain(_ links: [AmbitEngine.ChainLink]) -> [ItemRef] {
        links.dropLast().reversed().map { itemRef($0.item) }
    }

    static func route(_ reason: AmbitEngine.SelectionReason) -> Route {
        switch reason {
        case let .direct(entry): .direct(entry: selectionEntry(entry))
        case let .rule(entry): .rule(entry: selectionEntry(entry))
        case let .pack(pack, links): .pack(pack: itemRef(pack), chain: chain(links))
        case let .dependency(requirer, links): .dependency(requirer: itemRef(requirer), chain: chain(links))
        }
    }

    /// How `entry` reaches an item along `links` (entry first, the item last).
    static func route(entry: AmbitEngine.SelectionEntry, links: [AmbitEngine.ChainLink]) -> Route {
        let converted = selectionEntry(entry)
        guard links.count > 1 else {
            return converted.isRule ? .rule(entry: converted) : .direct(entry: converted)
        }
        let requirer = itemRef(links[links.count - 2].item)
        return requirer.kind == .pack
            ? .pack(pack: requirer, chain: chain(links)) : .dependency(requirer: requirer, chain: chain(links))
    }

    static func itemDetail(_ detail: AmbitEngine.ItemDetail, requires: [SelectionEntry]) -> ItemDetail {
        switch detail {
        case let .skill(path):
            .skill(path: path)
        case .pack:
            .pack(requires: requires)
        case let .mcp(_, transport):
            switch transport {
            case let .stdio(command, args, env):
                .mcp(transport: .stdio(command: command, args: args, envNames: env.map(\.name)))
            case let .http(url, _, headers):
                .mcp(transport: .http(url: url, headerNames: headers.map(\.name)))
            }
        case let .hook(_, event, matcher, hookType, command, timeoutSeconds):
            .hook(
                event: event, matcher: matcher, hookType: hookType == .script ? "script" : "command",
                command: command, timeout: timeoutSeconds.flatMap { UInt32(exactly: $0) })
        }
    }

    static func browseItem(_ item: AmbitEngine.BrowseItem) -> BrowseItem {
        let requires = item.requires.map(selectionEntry)
        return BrowseItem(
            item: ItemRef(kind: itemKind(item.kind), catalog: item.catalog, name: item.name),
            description: item.description, requires: requires,
            expects: item.prerequisites.map {
                switch $0 {
                case let .environmentVariable(name): name
                }
            },
            selected: item.selected, routes: item.reasons.map(route),
            detail: itemDetail(item.detail, requires: requires),
            limitations: item.limitations.map { ToolLimitation(tool: $0.tool, message: $0.message) })
    }

    static func browseResult(_ result: AmbitEngine.BrowseResult) -> BrowseResult {
        BrowseResult(items: result.items.map(browseItem), problems: result.problems.map(engineError))
    }

    static func skillDocument(_ document: AmbitEngine.SkillDocument) -> SkillDocument {
        SkillDocument(frontmatter: frontmatterFields(document.frontmatter), body: document.body, path: document.path)
    }

    /// The top-level `key: value` pairs of frontmatter YAML, in file order. A nested or multi-line
    /// value keeps its following lines, joined with newlines, as written.
    static func frontmatterFields(_ yaml: String) -> [FrontmatterField] {
        var fields: [FrontmatterField] = []
        for line in yaml.split(separator: "\n", omittingEmptySubsequences: false) {
            let isTopLevel = !(line.first?.isWhitespace ?? true) && !line.hasPrefix("#") && !line.hasPrefix("-")
            if isTopLevel, let colon = line.firstIndex(of: ":") {
                let key = line[..<colon].trimmingCharacters(in: .whitespaces)
                let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
                fields.append(FrontmatterField(key: key, value: value))
            } else if !fields.isEmpty, !line.trimmingCharacters(in: .whitespaces).isEmpty {
                let last = fields.count - 1
                let joined = fields[last].value.isEmpty ? String(line) : fields[last].value + "\n" + line
                fields[last].value = joined
            }
        }
        return fields
    }

    static func removalImpact(_ impact: AmbitEngine.RemovalImpact) -> RemovalImpact {
        RemovalImpact(
            item: itemRef(impact.item),
            sustaining: impact.sustaining.map {
                SustainingEntry(entry: selectionEntry($0.entry), routes: [route(entry: $0.entry, links: $0.chain)])
            },
            effects: impact.effects.map {
                EntryRemovalEffect(entry: selectionEntry($0.entry), diff: BundleDiff(removed: $0.removes.map(itemRef)))
            },
            removed: impact.removed.map(itemRef))
    }
}
