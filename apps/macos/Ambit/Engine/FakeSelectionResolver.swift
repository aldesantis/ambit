// A small stand-in for the engine's resolver, used by `FakeEngineService` when a fixture sets
// `resolvesSelection`. It follows `requires` entries, pack contents and skill dependencies over
// the fixture's items, so browsing, rule previews and removal impact react to the draft the way
// the real engine does. Patterns support `*` only; the real grammar is the engine's.

import Foundation

struct FakeSelectionResolver {
    let items: [BrowseItem]
    /// Catalogs whose entries can be judged. Entries for any other catalog are never unmatched.
    let loadedCatalogs: Set<String>

    init(items: [BrowseItem], loadedCatalogs: Set<String>? = nil) {
        self.items = items
        self.loadedCatalogs = loadedCatalogs ?? Set(items.map(\.item.catalog))
    }

    /// The items `entry` names. An unqualified entry looks in `catalog`, or in every catalog when
    /// that is `nil`.
    func matches(_ entry: SelectionEntry, in catalog: String? = nil) -> [BrowseItem] {
        let scope = entry.catalog ?? catalog
        return items.filter { candidate in
            candidate.item.kind == entry.kind && (scope == nil || candidate.item.catalog == scope)
                && Self.glob(entry.pattern, matches: candidate.item.name)
        }
    }

    /// Every selected item with its routes, ordered entries first, then packs, then skills.
    func routes(for entries: [SelectionEntry]) -> [ItemRef: [Route]] {
        var result: [ItemRef: [Route]] = [:]

        func add(_ route: Route, to item: ItemRef) {
            if result[item]?.contains(route) != true {
                result[item, default: []].append(route)
            }
        }

        // `chain` starts with `item` and walks back to the item an entry selected.
        func expand(_ item: BrowseItem, chain: [ItemRef]) {
            let dependencies: [SelectionEntry] =
                if case let .pack(requires) = item.detail { requires } else { item.requires }
            for dependency in dependencies {
                for target in matches(dependency, in: item.item.catalog) where !chain.contains(target.item) {
                    let route: Route =
                        item.item.kind == .pack
                        ? .pack(pack: item.item, chain: chain) : .dependency(requirer: item.item, chain: chain)
                    add(route, to: target.item)
                    expand(target, chain: [target.item] + chain)
                }
            }
        }

        for entry in entries {
            for match in matches(entry) {
                add(entry.isRule ? .rule(entry: entry) : .direct(entry: entry), to: match.item)
                expand(match, chain: [match.item])
            }
        }

        return result.mapValues { $0.sorted { $0.rank < $1.rank } }
    }

    /// The fixture's items with `selected` and `routes` computed from `entries`.
    func browse(_ entries: [SelectionEntry]) -> [BrowseItem] {
        let routes = routes(for: entries)
        return items.map { item in
            var item = item
            item.routes = routes[item.item] ?? []
            item.selected = !item.routes.isEmpty
            return item
        }
    }

    func packContents(_ pack: ItemRef) -> [ItemRef] {
        let entry = SelectionEntry(kind: .pack, catalog: pack.catalog, pattern: pack.name, isRule: false)
        return ordered(Set(routes(for: [entry]).keys))
    }

    func removalImpact(of item: ItemRef, entries: [SelectionEntry]) -> RemovalImpact {
        let selected = Set(routes(for: entries).keys)
        guard selected.contains(item) else {
            return RemovalImpact(item: item, sustaining: [], effects: [])
        }

        let sustaining = entries.compactMap { entry in
            routes(for: [entry])[item].map { SustainingEntry(entry: entry, routes: $0) }
        }

        func removedWithout(_ dropped: [SelectionEntry]) -> [ItemRef] {
            let remaining = Set(routes(for: entries.filter { !dropped.contains($0) }).keys)
            return ordered(selected.subtracting(remaining))
        }

        return RemovalImpact(
            item: item, sustaining: sustaining,
            effects: sustaining.map { EntryRemovalEffect(entry: $0.entry, diff: BundleDiff(removed: removedWithout([$0.entry]))) },
            removed: removedWithout(sustaining.map(\.entry)))
    }

    func unmatchedEntries(_ entries: [SelectionEntry]) -> [UnmatchedEntry] {
        entries.compactMap { entry in
            if let catalog = entry.catalog, !loadedCatalogs.contains(catalog) {
                return nil
            }
            guard matches(entry).isEmpty else {
                return nil
            }
            return UnmatchedEntry(entry: entry, error: Self.noMatch(entry))
        }
    }

    /// Mirrors the engine's checks: a pattern is a non-empty name without `/` or spaces.
    func previewRule(catalog: String, kind: ItemKind, pattern: String) throws -> [ItemRef] {
        if pattern.isEmpty || pattern.contains("/") || pattern.contains(where: \.isWhitespace) {
            throw EngineError.config(
                message: String(localized: "\"\(pattern)\" is not a valid pattern."),
                detail: [String(localized: "A pattern is a name, optionally with * wildcards.")], path: nil, line: nil)
        }

        let entry = SelectionEntry(kind: kind, catalog: catalog, pattern: pattern, isRule: pattern.contains("*"))
        let found = matches(entry).map(\.item)
        if found.isEmpty {
            throw Self.noMatch(entry)
        }
        return found
    }

    private func ordered(_ refs: Set<ItemRef>) -> [ItemRef] {
        items.map(\.item).filter(refs.contains)
    }

    private static func noMatch(_ entry: SelectionEntry) -> EngineError {
        let address = entry.catalog.map { "\($0)/\(entry.pattern)" } ?? entry.pattern
        return .resolution(message: String(localized: "No \(entry.kind.rawValue) matches \(address)."), detail: [])
    }

    static func glob(_ pattern: String, matches name: String) -> Bool {
        let parts = pattern.split(separator: "*", omittingEmptySubsequences: false).map(String.init)
        guard parts.count > 1 else {
            return pattern == name
        }

        var rest = Substring(name)
        guard let first = parts.first, rest.hasPrefix(first) else {
            return false
        }
        rest = rest.dropFirst(first.count)

        for part in parts.dropFirst().dropLast() where !part.isEmpty {
            guard let range = rest.range(of: part) else {
                return false
            }
            rest = rest[range.upperBound...]
        }

        let last = parts.last ?? ""
        return rest.count >= last.count && rest.hasSuffix(last)
    }
}

extension Route {
    /// Entries first, then packs, then skills, as the engine orders them.
    fileprivate var rank: Int {
        switch self {
        case .direct, .rule: 0
        case .pack: 1
        case .dependency: 2
        }
    }
}
