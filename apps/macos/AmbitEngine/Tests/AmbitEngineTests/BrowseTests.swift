import AmbitEngine
import Foundation
import Testing

/// Drives browsing through the bindings against a small local catalog.
struct BrowseTests {
    /// A temporary folder holding `catalog/` (one pack, two skills) and `project/ambit.yml`.
    private struct Fixture {
        let root: URL
        let project: URL
        let session: SetupSession

        init(requires: [String]) throws {
            root = FileManager.default.temporaryDirectory
                .appendingPathComponent("ambit-browse-\(UUID().uuidString)", isDirectory: true)
            project = root.appendingPathComponent("project", isDirectory: true)

            try Self.write(
                "---\nname: alpha\ndescription: The first skill.\n---\n\n# Alpha\n",
                to: root.appendingPathComponent("catalog/skills/alpha/SKILL.md"))
            try Self.write(
                "---\nname: beta\ndescription: The second skill.\n---\n\n# Beta\n",
                to: root.appendingPathComponent("catalog/skills/beta/SKILL.md"))
            try Self.write(
                "name: base\ndescription: The basics.\nrequires:\n  - skill: alpha\n",
                to: root.appendingPathComponent("catalog/packs/base.yml"))
            try Self.write(
                Self.config(requires: requires), to: project.appendingPathComponent("ambit.yml"))

            let home = root.appendingPathComponent("home").path
            let engine = Engine(config: EngineConfig(env: [
                "HOME": home,
                "XDG_CACHE_HOME": root.appendingPathComponent("cache").path,
            ]))
            session = engine.openSetup(root: project.path)
        }

        static func config(requires: [String]) -> String {
            let entries = requires.isEmpty
                ? " []" : requires.map { "\n  - \($0)" }.joined()
            return "version: 1\nharnesses: [claude]\ncatalogs:\n"
                + "  - name: company\n    source: path:../catalog\nrequires:\(entries)\n"
        }

        private static func write(_ text: String, to url: URL) throws {
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try text.write(to: url, atomically: true, encoding: .utf8)
        }

        func remove() {
            try? FileManager.default.removeItem(at: root)
        }
    }

    @Test func loadsAndListsEveryItem() throws {
        let fixture = try Fixture(requires: [#"pack: "company/base""#])
        defer { fixture.remove() }

        let state = try fixture.session.loadCatalogs(draftText: nil, policy: .cacheOnly)
        #expect(state.catalogs.map(\.name) == ["company"])
        #expect(state.catalogs.first?.state == .loaded(commit: nil, local: true))

        let result = try fixture.session.browse(draftText: nil)
        #expect(result.problems.isEmpty)
        #expect(result.items.map(\.name) == ["base", "alpha", "beta"])

        let alpha = try #require(result.items.first { $0.name == "alpha" })
        #expect(alpha.selected)
        guard case let .pack(pack, chain) = try #require(alpha.reasons.first) else {
            Issue.record("expected alpha to be selected by its pack, got \(alpha.reasons)")
            return
        }
        #expect(pack == ItemRef(kind: .pack, catalog: "company", name: "base"))
        #expect(chain.count == 2)
    }

    @Test func readsDetailsAndPreviewsRules() throws {
        let fixture = try Fixture(requires: [])
        defer { fixture.remove() }

        let document = try fixture.session.skillDocument(catalog: "company", name: "beta")
        #expect(document.body.contains("# Beta"))

        let contents = try fixture.session.packContents(catalog: "company", name: "base")
        #expect(contents.map(\.name) == ["base", "alpha"])

        let matches = try fixture.session.previewRule(
            draftText: nil, catalog: "company", kind: .skill, pattern: "*a")
        #expect(matches.map(\.name) == ["alpha", "beta"])

        #expect(throws: EngineError.self) {
            _ = try fixture.session.previewRule(
                draftText: nil, catalog: "company", kind: .skill, pattern: "zzz*")
        }
    }

    @Test func explainsWhatAnUninstallTakes() throws {
        let fixture = try Fixture(requires: [])
        defer { fixture.remove() }
        let draft = Fixture.config(requires: [#"pack: "company/base""#, #"skill: "company/alpha""#])

        let impact = try fixture.session.removalImpact(
            draftText: draft, item: ItemRef(kind: .skill, catalog: "company", name: "alpha"))

        #expect(impact.sustaining.map(\.entry.pattern) == ["base", "alpha"])
        #expect(impact.removed.map(\.name) == ["base", "alpha"])
        #expect(try fixture.session.unmatchedEntries(draftText: draft).isEmpty)
    }
}
