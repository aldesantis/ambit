// Selection edits the Capabilities area stages in the setup's draft. Browsing state lives in
// `CapabilitiesModel`; this file only turns user intents into `ConfigEdit`s.

import Foundation

extension SetupModel {
    /// Changes whenever the items to browse can change: a new draft text, a re-read config, or
    /// the end of an operation that may have loaded catalogs into the cache.
    struct CapabilitiesReloadKey: Hashable {
        var draftText: String?
        var config: ConfigState?
        var isIdle: Bool
    }

    var capabilitiesReloadKey: CapabilitiesReloadKey {
        CapabilitiesReloadKey(draftText: draftText, config: snapshot?.config, isIdle: !operations.isBusy)
    }

    /// Stages a direct, catalog-qualified selection of `item`.
    func stageSelection(of item: ItemRef) throws {
        try stage([.addEntry(.direct(item))])
    }

    /// Stages the removal of exactly `entries`. Nothing else is removed with them.
    func stageRemoval(of entries: [SelectionEntry]) throws {
        try stage(entries.map(ConfigEdit.removeEntry))
    }

    /// Stages a new rule, or replaces `original` with it in place.
    func stageRule(_ rule: SelectionEntry, replacing original: SelectionEntry?) throws {
        guard let original else {
            try stage([.addEntry(rule)])
            return
        }
        guard original != rule else {
            return
        }
        try stage([.replaceEntry(old: original, new: rule)])
    }
}

extension SelectionEntry {
    /// The entry that selects exactly `item`, qualified with its catalog.
    static func direct(_ item: ItemRef) -> SelectionEntry {
        SelectionEntry(kind: item.kind, catalog: item.catalog, pattern: item.name, isRule: false)
    }

    /// True when the entry names `item` itself rather than matching it with a wildcard.
    func names(_ item: ItemRef) -> Bool {
        !isRule && kind == item.kind && pattern == item.name && (catalog == nil || catalog == item.catalog)
    }

    /// The entry as written in `requires`, such as `skill: team/review`.
    var displayText: String {
        "\(kind.rawValue): \(catalog.map { "\($0)/\(pattern)" } ?? pattern)"
    }
}
