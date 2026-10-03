// Named fixtures for UI tests, chosen with `AMBIT_TEST_ENGINE=<scenario>`. Without a scenario,
// or with an unknown one, the fake starts empty.

import Foundation

extension FakeEngineService {
    static func scenario(_ name: String?, home: URL) -> FakeEngineService {
        switch name {
        case "capabilities":
            FakeEngineService(fixtures: [home.path: capabilitiesFixture(root: home.path)])
        case "catalogUpdates":
            FakeEngineService(fixtures: [home.path: catalogUpdatesFixture(root: home.path)])
        default:
            FakeEngineService()
        }
    }

    /// A Personal setup with one local catalog holding every item kind and nothing selected.
    static func capabilitiesFixture(root: String) -> Fixture {
        let text = """
            version: 1
            harnesses:
              - claude
            catalogs:
              - name: team
                source: ./catalog
            requires: []

            """

        func ref(_ kind: ItemKind, _ name: String) -> ItemRef {
            ItemRef(kind: kind, catalog: "team", name: name)
        }
        func skill(_ name: String) -> SelectionEntry {
            SelectionEntry(kind: .skill, catalog: nil, pattern: name, isRule: false)
        }

        let items = [
            BrowseItem(
                item: ref(.pack, "starter"), description: "Everything a new project needs.", requires: [],
                expects: [], selected: false, routes: [], detail: .pack(requires: [skill("review")])),
            BrowseItem(
                item: ref(.skill, "lint"), description: "Runs the project's linters.", requires: [], expects: [],
                selected: false, routes: [], detail: .skill(path: "skills/lint")),
            BrowseItem(
                item: ref(.skill, "notes"), description: nil, requires: [], expects: [], selected: false,
                routes: [], detail: .skill(path: "skills/notes")),
            BrowseItem(
                item: ref(.skill, "review"), description: "Reviews pull requests.", requires: [skill("lint")],
                expects: ["GITHUB_TOKEN"], selected: false, routes: [], detail: .skill(path: "skills/review")),
            BrowseItem(
                item: ref(.mcp, "github"), description: "GitHub tools.", requires: [], expects: ["GITHUB_TOKEN"],
                selected: false, routes: [],
                detail: .mcp(
                    transport: .stdio(command: "npx", args: ["-y", "github-mcp"], envNames: ["GITHUB_TOKEN"]))),
            BrowseItem(
                item: ref(.hook, "format"), description: "Formats edited files.", requires: [], expects: [],
                selected: false, routes: [],
                detail: .hook(
                    event: "PostToolUse", matcher: "Edit", hookType: "script", command: "format.sh", timeout: 30),
                limitations: [ToolLimitation(tool: "codex", message: "Codex has no hooks.")]),
        ]

        let review = SkillDocument(
            frontmatter: [
                FrontmatterField(key: "name", value: "review"),
                FrontmatterField(key: "description", value: "Reviews pull requests."),
            ],
            body: """
                # Review

                Read the **diff** first, then run `lint`. See [the guide](https://example.com/guide).

                - Check tests
                - Check docs

                <script>alert("never runs")</script>
                """,
            path: "skills/review/SKILL.md")

        return Fixture(
            config: (try? validConfig(text, root: root)) ?? .missing,
            catalogs: [CatalogLoadState(name: "team", availability: .available(commit: nil, local: true))],
            items: items, resolvesSelection: true, documents: [ref(.skill, "review"): review])
    }
}
