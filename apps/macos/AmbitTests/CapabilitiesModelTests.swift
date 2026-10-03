import Foundation
import Testing

@testable import Ambit

@MainActor
struct CapabilitiesModelTests {
    private static let root = "/tmp/ambit-capabilities-tests"

    private static func ref(_ kind: ItemKind, _ name: String, catalog: String = "team") -> ItemRef {
        ItemRef(kind: kind, catalog: catalog, name: name)
    }

    private static func entry(_ kind: ItemKind, _ pattern: String, catalog: String? = "team") -> SelectionEntry {
        SelectionEntry(kind: kind, catalog: catalog, pattern: pattern, isRule: pattern.contains("*"))
    }

    private static func item(
        _ kind: ItemKind, _ name: String, catalog: String = "team", description: String? = nil,
        requires: [String] = [], pack: [SelectionEntry] = []
    ) -> BrowseItem {
        let detail: ItemDetail =
            switch kind {
            case .skill: .skill(path: "skills/\(name)")
            case .pack: .pack(requires: pack)
            case .mcp: .mcp(transport: .http(url: "https://example.com/mcp", headerNames: []))
            case .hook: .hook(event: "PostToolUse", matcher: nil, hookType: "command", command: "true", timeout: nil)
            }
        return BrowseItem(
            item: ref(kind, name, catalog: catalog), description: description,
            requires: requires.map { entry(.skill, $0, catalog: nil) }, expects: [], selected: false, routes: [],
            detail: detail)
    }

    /// `review` and `audit` both require `lint`; `starter` packs `review`; `other` has its own skill.
    private static let items = [
        item(.pack, "starter", pack: [entry(.skill, "review", catalog: nil)]),
        item(.skill, "audit", description: "Audits dependencies.", requires: ["lint"]),
        item(.skill, "lint", description: "Runs linters."),
        item(.skill, "notes"),
        item(.skill, "review", description: "Reviews pull requests.", requires: ["lint"]),
        item(.mcp, "github", description: "GitHub tools."),
        item(.hook, "format"),
        item(.skill, "deploy", catalog: "other", description: "Ships it."),
    ]

    private func makeModel(
        requires: [SelectionEntry], catalogs: [CatalogLoadState]? = nil, items: [BrowseItem] = items
    ) async throws -> (CapabilitiesModel, SetupModel) {
        let engine = FakeEngineService()
        let summary = ConfigSummary(
            harnesses: ["claude"],
            catalogs: [
                CatalogEntry(name: "team", source: "./team", gitRef: nil, sourceKind: .local(path: "./team")),
                CatalogEntry(name: "other", source: "./other", gitRef: nil, sourceKind: .local(path: "./other")),
            ],
            requires: requires)
        let text = FakeConfigText.render(summary)
        engine.setFixture(
            FakeEngineService.Fixture(
                config: try FakeEngineService.validConfig(text, root: Self.root),
                catalogs: catalogs ?? [
                    CatalogLoadState(name: "team", availability: .available(commit: nil, local: true)),
                    CatalogLoadState(name: "other", availability: .available(commit: nil, local: true)),
                ],
                items: items, resolvesSelection: true),
            root: Self.root)

        let setup = SetupModel(
            id: .personal, root: URL(fileURLWithPath: Self.root), engine: engine, operations: OperationRunner())
        await setup.refresh()
        let model = CapabilitiesModel(setup: setup)
        await model.reload()
        return (model, setup)
    }

    // MARK: Browsing and filters

    @Test func listsEveryKindIncludingUnselected() async throws {
        let (model, _) = try await makeModel(requires: [])

        #expect(model.items.count == Self.items.count)
        #expect(Set(model.items.map(\.item.kind)) == Set(ItemKind.allCases))
        #expect(model.items.allSatisfy { !$0.selected })
        #expect(model.loadError == nil)
    }

    @Test func searchMatchesNamesAndDescriptions() async throws {
        let (model, _) = try await makeModel(requires: [])

        model.searchText = "pull request"
        #expect(model.filteredItems.map(\.item.name) == ["review"])

        model.searchText = "LINT"
        #expect(model.filteredItems.map(\.item.name) == ["lint"])
    }

    @Test func filtersByCatalogKindAndSelection() async throws {
        let (model, _) = try await makeModel(requires: [Self.entry(.skill, "review")])

        model.catalogFilter = "other"
        #expect(model.filteredItems.map(\.item.name) == ["deploy"])

        model.catalogFilter = nil
        model.kindFilter = .mcp
        #expect(model.filteredItems.map(\.item.name) == ["github"])

        model.kindFilter = .skill
        model.selectionFilter = .selected
        #expect(Set(model.filteredItems.map(\.item.name)) == ["review", "lint"])

        model.selectionFilter = .unselected
        #expect(Set(model.filteredItems.map(\.item.name)) == ["audit", "notes", "deploy"])
    }

    @Test func showsEveryReasonOfAnItem() async throws {
        let (model, _) = try await makeModel(requires: [
            Self.entry(.skill, "review"), Self.entry(.pack, "starter"), Self.entry(.skill, "l*"),
        ])

        let review = try #require(model.item(for: Self.ref(.skill, "review")))
        #expect(CapabilitiesModel.reasons(of: review) == [.direct, .pack])

        let lint = try #require(model.item(for: Self.ref(.skill, "lint")))
        #expect(CapabilitiesModel.reasons(of: lint) == [.rule, .dependency])
    }

    @Test func notCachedCatalogsAreReported() async throws {
        let (model, _) = try await makeModel(
            requires: [],
            catalogs: [
                CatalogLoadState(name: "team", availability: .available(commit: nil, local: true)),
                CatalogLoadState(name: "other", availability: .notCached),
            ],
            items: Self.items.filter { $0.item.catalog == "team" })

        #expect(model.unavailableCatalogs.map(\.name) == ["other"])
        #expect(!model.items.contains { $0.item.catalog == "other" })
    }

    // MARK: Selecting

    @Test func selectingStagesADirectEntry() async throws {
        let (model, setup) = try await makeModel(requires: [])

        await model.select(Self.ref(.skill, "review"))

        #expect(setup.hasPendingChanges)
        #expect(setup.configSummary?.requires == [Self.entry(.skill, "review")])
        let review = try #require(model.item(for: Self.ref(.skill, "review")))
        #expect(review.routes == [.direct(entry: Self.entry(.skill, "review"))])
        let lint = try #require(model.item(for: Self.ref(.skill, "lint")))
        #expect(CapabilitiesModel.reasons(of: lint) == [.dependency])
    }

    // MARK: Removing

    @Test func directOnlyRemovalIsStagedAtOnce() async throws {
        let (model, setup) = try await makeModel(requires: [Self.entry(.skill, "notes")])

        await model.requestRemoval(of: Self.ref(.skill, "notes"))

        #expect(model.removal == nil)
        #expect(setup.configSummary?.requires == [])
        #expect(model.item(for: Self.ref(.skill, "notes"))?.selected == false)
    }

    @Test func packMemberOffersThePackEntryWithEveryAffectedItem() async throws {
        let (model, setup) = try await makeModel(requires: [Self.entry(.pack, "starter")])

        await model.requestRemoval(of: Self.ref(.skill, "review"))

        let plan = try #require(model.removal)
        #expect(plan.steps.map(\.reason) == [.pack])
        #expect(plan.steps.first?.entry == Self.entry(.pack, "starter"))
        #expect(
            Set(plan.steps.first?.removes ?? [])
                == [Self.ref(.pack, "starter"), Self.ref(.skill, "review"), Self.ref(.skill, "lint")])
        // Nothing is staged until the user picks the entry.
        #expect(!setup.hasPendingChanges)

        await model.removeStep(try #require(plan.steps.first))

        #expect(model.removal?.isComplete == true)
        #expect(setup.configSummary?.requires == [])
    }

    @Test func ruleMatchOffersTheRuleAndKeepsTheDirectEntryHonest() async throws {
        let (model, setup) = try await makeModel(requires: [Self.entry(.skill, "notes"), Self.entry(.skill, "n*")])

        await model.requestRemoval(of: Self.ref(.skill, "notes"))

        let plan = try #require(model.removal)
        #expect(plan.steps.map(\.reason) == [.direct, .rule])

        // Removing the direct entry leaves the rule: the plan must not call the item uninstalled.
        await model.removeStep(try #require(plan.steps.first { $0.reason == .direct }))

        let remaining = try #require(model.removal)
        #expect(!remaining.isComplete)
        #expect(remaining.steps.map(\.entry) == [Self.entry(.skill, "n*")])
        #expect(model.item(for: Self.ref(.skill, "notes"))?.selected == true)
        #expect(setup.configSummary?.requires == [Self.entry(.skill, "n*")])
    }

    @Test func dependencyRequiresRemovingEachSustainingSelectionAndKeepsSharedOnes() async throws {
        let (model, setup) = try await makeModel(requires: [Self.entry(.skill, "review"), Self.entry(.skill, "audit")])

        await model.requestRemoval(of: Self.ref(.skill, "lint"))

        let plan = try #require(model.removal)
        #expect(plan.steps.map(\.reason) == [.dependency, .dependency])
        // Removing only one requirer would keep the shared dependency.
        let reviewStep = try #require(plan.steps.first { $0.entry == Self.entry(.skill, "review") })
        #expect(reviewStep.removes == [Self.ref(.skill, "review")])
        #expect(Set(plan.removed) == [Self.ref(.skill, "review"), Self.ref(.skill, "audit"), Self.ref(.skill, "lint")])
        #expect(!setup.hasPendingChanges)

        await model.removeStep(reviewStep)

        // No cascade: audit stays selected and still keeps lint.
        #expect(setup.configSummary?.requires == [Self.entry(.skill, "audit")])
        let lint = try #require(model.item(for: Self.ref(.skill, "lint")))
        #expect(lint.selected)
        #expect(
            lint.routes == [.dependency(requirer: Self.ref(.skill, "audit"), chain: [Self.ref(.skill, "audit")])])
        let remaining = try #require(model.removal)
        #expect(!remaining.isComplete)
        #expect(remaining.staged == [Self.entry(.skill, "review")])

        await model.removeStep(try #require(remaining.steps.first))

        #expect(model.removal?.isComplete == true)
        #expect(setup.configSummary?.requires == [])
    }

    @Test func notSelectedItemOpensNoPlan() async throws {
        let (model, setup) = try await makeModel(requires: [])

        await model.requestRemoval(of: Self.ref(.skill, "notes"))

        #expect(model.removal == nil)
        #expect(!setup.hasPendingChanges)
    }

    // MARK: Rules

    @Test func rulePreviewShowsMatchesOrTheEngineError() async throws {
        let (model, _) = try await makeModel(requires: [])

        model.startNewRule()
        model.ruleDraft?.catalog = "team"
        model.ruleDraft?.pattern = "a/b"
        await model.previewRuleDraft()
        guard case let .invalid(error)? = model.ruleDraft?.preview, case .config = error else {
            Issue.record("Expected a validation error, got \(String(describing: model.ruleDraft?.preview))")
            return
        }
        #expect(model.ruleDraft?.canSave == false)

        model.ruleDraft?.pattern = "zzz*"
        await model.previewRuleDraft()
        guard case let .invalid(error)? = model.ruleDraft?.preview, case .resolution = error else {
            Issue.record("Expected a no-match error, got \(String(describing: model.ruleDraft?.preview))")
            return
        }

        model.ruleDraft?.pattern = "*i*"
        await model.previewRuleDraft()
        #expect(
            model.ruleDraft?.preview
                == .matches([Self.ref(.skill, "audit"), Self.ref(.skill, "lint"), Self.ref(.skill, "review")]))
        #expect(model.ruleDraft?.canSave == true)
    }

    @Test func savingEditingAndRemovingARule() async throws {
        let (model, setup) = try await makeModel(requires: [])

        model.startNewRule()
        model.ruleDraft?.catalog = "team"
        model.ruleDraft?.pattern = "re*"
        await model.previewRuleDraft()
        await model.saveRuleDraft()

        #expect(model.ruleDraft == nil)
        #expect(model.rules == [Self.entry(.skill, "re*")])
        #expect(model.item(for: Self.ref(.skill, "review"))?.routes.first == .rule(entry: Self.entry(.skill, "re*")))

        model.startEditing(Self.entry(.skill, "re*"))
        model.ruleDraft?.pattern = "no*"
        await model.previewRuleDraft()
        await model.saveRuleDraft()

        #expect(setup.configSummary?.requires == [Self.entry(.skill, "no*")])
        #expect(model.item(for: Self.ref(.skill, "review"))?.selected == false)

        await model.removeEntry(Self.entry(.skill, "no*"))
        #expect(model.rules.isEmpty)
        #expect(setup.configSummary?.requires == [])
    }

    @Test func unmatchedEntriesAreReported() async throws {
        let (model, _) = try await makeModel(requires: [Self.entry(.skill, "gone*"), Self.entry(.skill, "notes")])

        #expect(model.unmatched.map(\.entry) == [Self.entry(.skill, "gone*")])
        #expect(model.unmatchedError(for: Self.entry(.skill, "gone*")) != nil)
    }

    @Test func entriesOfUnloadedCatalogsAreNotJudged() async throws {
        let (model, _) = try await makeModel(
            requires: [Self.entry(.skill, "deploy", catalog: "other")],
            catalogs: [
                CatalogLoadState(name: "team", availability: .available(commit: nil, local: true)),
                CatalogLoadState(name: "other", availability: .notCached),
            ],
            items: Self.items.filter { $0.item.catalog == "team" })

        #expect(model.unmatched.isEmpty)
    }
}

struct MarkdownRendererTests {
    @Test func rendersBlocksWithoutExecutingOrLoadingAnything() throws {
        let blocks = MarkdownRenderer.blocks(
            from: """
                # Title

                Text with [web](https://example.com), [script](javascript:alert(1)) and [file](file:///etc/passwd).

                ![logo](https://example.com/logo.png)

                <script>alert(1)</script>
                """)

        #expect(blocks.count == 4)
        guard case let .heading(level, title) = blocks[0] else {
            Issue.record("Expected a heading")
            return
        }
        #expect(level == 1)
        #expect(String(title.characters) == "Title")

        guard case let .paragraph(text) = blocks[1] else {
            Issue.record("Expected a paragraph")
            return
        }
        let links = text.runs.compactMap(\.link)
        #expect(links == [URL(string: "https://example.com")!])

        guard case let .paragraph(image) = blocks[2] else {
            Issue.record("Expected the image as text")
            return
        }
        #expect(String(image.characters) == "[image: logo]")
        #expect(image.runs.allSatisfy { $0.link == nil })

        #expect(blocks[3] == .literal("<script>alert(1)</script>"))
    }

    @Test func onlyWebAndMailLinksAreSafe() {
        #expect(MarkdownRenderer.safeURL("https://a.b") != nil)
        #expect(MarkdownRenderer.safeURL("mailto:a@b.c") != nil)
        #expect(MarkdownRenderer.safeURL("javascript:alert(1)") == nil)
        #expect(MarkdownRenderer.safeURL("file:///etc") == nil)
        #expect(MarkdownRenderer.safeURL("relative/path.md") == nil)
    }
}
