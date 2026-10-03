// Browsing and selecting one setup's capabilities. Everything is computed by the engine against
// the setup's draft text (or the saved config), so what the list shows is what Apply would
// install. Catalogs are loaded cache-only: this area never fetches. A catalog missing from the
// cache is shown with its state, and the Catalogs area offers the fetch.
//
// Uninstalling follows the item's selection routes from `removalImpact`. An item kept only by
// entries that name it is removed at once. Any other route (a rule, a pack, a dependency) opens
// a `RemovalPlan` that lists every sustaining entry; each one is staged only when the user picks
// it, so nothing cascades, and the plan says the item will be uninstalled only once no entry
// reaches it any more.

import Foundation
import Observation

@MainActor
@Observable
final class CapabilitiesModel {
    enum SelectionFilter: String, CaseIterable, Identifiable {
        case all, selected, unselected

        var id: Self { self }
    }

    /// One kind of selection route, as the list badges it.
    enum Reason: Int, CaseIterable, Comparable, Hashable {
        case direct, rule, pack, dependency

        static func < (lhs: Self, rhs: Self) -> Bool { lhs.rawValue < rhs.rawValue }

        init(_ route: Route) {
            switch route {
            case .direct: self = .direct
            case .rule: self = .rule
            case .pack: self = .pack
            case .dependency: self = .dependency
            }
        }
    }

    /// One entry that keeps an item selected, and what removing only that entry would uninstall.
    struct RemovalStep: Identifiable, Hashable {
        var entry: SelectionEntry
        /// How the entry reaches the item. Decides what the plan offers: remove a direct entry,
        /// edit or remove a rule, remove a pack entry, or remove the selection a dependency
        /// comes from.
        var reason: Reason
        var routes: [Route]
        var removes: [ItemRef]

        var id: SelectionEntry { entry }
    }

    /// What uninstalling `item` takes when its own direct entries are not the only route.
    struct RemovalPlan: Identifiable, Hashable {
        var item: ItemRef
        var steps: [RemovalStep]
        /// What removing every remaining step would uninstall.
        var removed: [ItemRef]
        /// Entries removed through this plan so far.
        var staged: [SelectionEntry] = []

        var id: ItemRef { item }

        /// True once no entry reaches the item: Apply will uninstall it.
        var isComplete: Bool { steps.isEmpty }
    }

    /// The rule form: a catalog, a kind and a pattern, previewed live against the draft.
    struct RuleDraft: Identifiable, Hashable {
        enum Preview: Hashable {
            case idle
            case loading
            case matches([ItemRef])
            case invalid(EngineError)
        }

        let id = UUID()
        /// The rule being edited, `nil` for a new one.
        var original: SelectionEntry?
        var catalog: String
        var kind: ItemKind
        var pattern: String
        var preview: Preview = .idle

        var entry: SelectionEntry {
            SelectionEntry(kind: kind, catalog: catalog, pattern: pattern, isRule: pattern.contains("*"))
        }

        /// Only a rule the engine accepted and that matches something can be saved.
        var canSave: Bool {
            if case let .matches(found) = preview { !found.isEmpty } else { false }
        }
    }

    let setup: SetupModel

    private(set) var catalogs: [CatalogLoadState] = []
    private(set) var items: [BrowseItem] = []
    /// Resolution problems that keep the selection from resolving at all.
    private(set) var problems: [EngineError] = []
    private(set) var unmatched: [UnmatchedEntry] = []
    private(set) var loadError: EngineError?
    private(set) var isLoading = false
    private(set) var hasLoaded = false

    var searchText = ""
    var catalogFilter: String?
    var kindFilter: ItemKind?
    var selectionFilter: SelectionFilter = .all
    /// The item shown in the detail pane.
    var selection: ItemRef?

    private(set) var skillDocument: SkillDocument?
    private(set) var packContents: [ItemRef]?
    private(set) var detailError: EngineError?

    var removal: RemovalPlan?
    var ruleDraft: RuleDraft?
    var actionError: PresentedError?

    @ObservationIgnored private var generation = 0
    /// A rule to open in the rule form once the removal sheet has closed.
    @ObservationIgnored private var ruleToEditAfterRemoval: SelectionEntry?

    init(setup: SetupModel) {
        self.setup = setup
    }

    /// False when the setup has no config to browse with: missing, invalid or ambiguous.
    var canBrowse: Bool { setup.configSummary != nil }

    var configuredCatalogs: [String] { setup.configSummary?.catalogs.map(\.name) ?? [] }

    var rules: [SelectionEntry] { setup.configSummary?.requires.filter(\.isRule) ?? [] }

    /// Catalogs whose items are not listed, with why.
    var unavailableCatalogs: [CatalogLoadState] {
        catalogs.filter { if case .available = $0.availability { false } else { true } }
    }

    var filteredItems: [BrowseItem] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        return items.filter { item in
            if let catalogFilter, item.item.catalog != catalogFilter {
                return false
            }
            if let kindFilter, item.item.kind != kindFilter {
                return false
            }
            switch selectionFilter {
            case .all: break
            case .selected where !item.selected: return false
            case .unselected where item.selected: return false
            default: break
            }
            guard !query.isEmpty else {
                return true
            }
            return item.item.name.localizedStandardContains(query)
                || (item.description?.localizedStandardContains(query) ?? false)
        }
    }

    var selectedItem: BrowseItem? {
        selection.flatMap(item(for:))
    }

    func item(for ref: ItemRef) -> BrowseItem? {
        items.first { $0.item == ref }
    }

    func unmatchedError(for entry: SelectionEntry) -> EngineError? {
        unmatched.first { $0.entry == entry }?.error
    }

    /// Every distinct route kind of `item`, in badge order.
    static func reasons(of item: BrowseItem) -> [Reason] {
        Array(Set(item.routes.map(Reason.init))).sorted()
    }

    /// True when an entry in the config names `item` itself.
    func isDirectlySelected(_ item: BrowseItem) -> Bool {
        item.routes.contains { if case .direct = $0 { true } else { false } }
    }

    // MARK: Loading

    /// Loads the catalogs from the cache and browses them against the draft.
    func reload() async {
        generation += 1
        let current = generation

        guard canBrowse else {
            catalogs = []
            items = []
            problems = []
            unmatched = []
            loadError = nil
            hasLoaded = true
            return
        }

        let draftText = setup.draftText
        isLoading = true
        defer {
            if current == generation {
                isLoading = false
            }
        }

        do {
            let state = try await setup.session.loadCatalogs(draftText: draftText, policy: .cacheOnly, progress: nil)
            let result = try await setup.session.browse(draftText: draftText)
            let unmatched = try await setup.session.unmatchedEntries(draftText: draftText)
            guard current == generation else {
                return
            }

            catalogs = state.catalogs
            items = result.items
            problems = result.problems
            self.unmatched = unmatched
            loadError = nil
        } catch {
            guard current == generation, !error.isCancellation else {
                return
            }
            loadError = EngineError(error)
        }
        hasLoaded = true

        if let removal {
            await refreshRemoval(of: removal.item, staged: removal.staged)
        }
    }

    /// Loads what the detail pane shows beyond the browse result: a skill's `SKILL.md` or a
    /// pack's resolved contents.
    func loadDetail() async {
        skillDocument = nil
        packContents = nil
        detailError = nil
        guard let ref = selection else {
            return
        }

        do {
            switch ref.kind {
            case .skill:
                let document = try await setup.session.skillDocument(catalog: ref.catalog, name: ref.name)
                if selection == ref {
                    skillDocument = document
                }
            case .pack:
                let contents = try await setup.session.packContents(catalog: ref.catalog, name: ref.name)
                if selection == ref {
                    packContents = contents
                }
            case .mcp, .hook:
                break
            }
        } catch {
            if selection == ref, !error.isCancellation {
                detailError = EngineError(error)
            }
        }
    }

    // MARK: Selecting

    /// Stages a direct selection of `item`.
    func select(_ item: ItemRef) async {
        await perform(String(localized: "Could not select \(item.name).")) {
            try setup.stageSelection(of: item)
        }
    }

    // MARK: Removing

    /// Starts uninstalling `item`. Removes its own direct entries when nothing else selects it;
    /// otherwise opens a `RemovalPlan`.
    func requestRemoval(of item: ItemRef) async {
        let impact: RemovalImpact
        do {
            impact = try await setup.session.removalImpact(draftText: setup.draftText, item: item)
        } catch {
            actionError = PresentedError(title: String(localized: "Could not work out how to remove \(item.name)."), error: error)
            return
        }

        let plan = Self.plan(for: impact)
        if plan.isComplete {
            return
        }
        if plan.steps.allSatisfy({ $0.reason == .direct }) {
            await perform(String(localized: "Could not remove \(item.name).")) {
                try setup.stageRemoval(of: plan.steps.map(\.entry))
            }
            return
        }

        removal = plan
    }

    /// Stages the removal of one sustaining entry of the open plan, then re-reads what still
    /// keeps the item selected.
    func removeStep(_ step: RemovalStep) async {
        guard removal != nil else {
            return
        }

        do {
            try setup.stageRemoval(of: [step.entry])
        } catch {
            actionError = PresentedError(title: String(localized: "Could not remove \(step.entry.displayText)."), error: error)
            return
        }

        removal?.staged.append(step.entry)
        // Reloading also re-reads the plan.
        await reload()
    }

    private func refreshRemoval(of item: ItemRef, staged: [SelectionEntry]) async {
        do {
            let impact = try await setup.session.removalImpact(draftText: setup.draftText, item: item)
            var plan = Self.plan(for: impact)
            plan.staged = staged
            if removal?.item == item {
                removal = plan
            }
        } catch {
            actionError = PresentedError(title: String(localized: "Could not work out how to remove \(item.name)."), error: error)
        }
    }

    /// Classifies each sustaining entry by how it reaches the item.
    static func plan(for impact: RemovalImpact) -> RemovalPlan {
        let steps = impact.sustaining.map { sustaining in
            let entry = sustaining.entry
            let reason: Reason =
                if entry.isRule {
                    .rule
                } else if entry.names(impact.item) {
                    .direct
                } else if entry.kind == .pack {
                    .pack
                } else {
                    .dependency
                }
            let removes = impact.effects.first { $0.entry == entry }?.diff.removed ?? []
            return RemovalStep(entry: entry, reason: reason, routes: sustaining.routes, removes: removes)
        }
        return RemovalPlan(item: impact.item, steps: steps, removed: impact.removed)
    }

    /// Closes the removal plan and opens the rule form on `rule`, one sheet after the other.
    func editRuleAfterRemoval(_ rule: SelectionEntry) {
        ruleToEditAfterRemoval = rule
        removal = nil
    }

    func removalDismissed() {
        if let rule = ruleToEditAfterRemoval {
            ruleToEditAfterRemoval = nil
            startEditing(rule)
        }
    }

    // MARK: Rules

    func startNewRule() {
        let catalog = catalogFilter ?? configuredCatalogs.first ?? ""
        ruleDraft = RuleDraft(original: nil, catalog: catalog, kind: kindFilter ?? .skill, pattern: "")
    }

    /// Opens the rule form on `rule`. An unqualified rule is edited with the first catalog.
    func startEditing(_ rule: SelectionEntry) {
        ruleDraft = RuleDraft(
            original: rule, catalog: rule.catalog ?? configuredCatalogs.first ?? "", kind: rule.kind,
            pattern: rule.pattern)
    }

    /// Asks the engine what the open rule form matches. Errors are the engine's validation.
    func previewRuleDraft() async {
        guard let draft = ruleDraft else {
            return
        }

        let pattern = draft.pattern.trimmingCharacters(in: .whitespaces)
        guard !pattern.isEmpty, !draft.catalog.isEmpty else {
            ruleDraft?.preview = .idle
            return
        }

        ruleDraft?.preview = .loading
        let preview: RuleDraft.Preview
        do {
            preview = .matches(
                try await setup.session.previewRule(
                    draftText: setup.draftText, catalog: draft.catalog, kind: draft.kind, pattern: pattern))
        } catch {
            guard !error.isCancellation else {
                return
            }
            preview = .invalid(EngineError(error))
        }

        if ruleDraft?.id == draft.id, ruleDraft?.entry == draft.entry {
            ruleDraft?.preview = preview
        }
    }

    /// Stages the open rule form and closes it.
    func saveRuleDraft() async {
        guard let draft = ruleDraft, draft.canSave else {
            return
        }

        var entry = draft.entry
        entry.pattern = entry.pattern.trimmingCharacters(in: .whitespaces)
        await perform(String(localized: "Could not save the rule.")) {
            try setup.stageRule(entry, replacing: draft.original)
        }
        if actionError == nil {
            ruleDraft = nil
        }
    }

    /// Stages the removal of `entry`, a rule or an entry that matches nothing.
    func removeEntry(_ entry: SelectionEntry) async {
        await perform(String(localized: "Could not remove \(entry.displayText).")) {
            try setup.stageRemoval(of: [entry])
        }
    }

    // MARK: Helpers

    private func perform(_ failure: String, _ edit: () throws -> Void) async {
        do {
            try edit()
        } catch {
            actionError = PresentedError(title: failure, error: error)
            return
        }
        await reload()
    }
}
