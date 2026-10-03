import Foundation
import Testing

@testable import Ambit

@MainActor
struct CatalogsModelTests {
    private let root: URL
    private let engine = FakeEngineService()

    private static let config = """
        version: 1
        harnesses:
          - claude
        catalogs:
          - name: team
            source: ./team-catalog
          - name: remote
            source: acme/catalog
        requires:
          - skill: team/review
          - skill: team/lint-*
          - mcp: remote/github
        """

    init() throws {
        root = FileManager.default.temporaryDirectory
            .appending(path: "ambit-catalogs-\(UUID().uuidString)", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        engine.setFixture(
            FakeEngineService.Fixture(config: try FakeEngineService.validConfig(Self.config, root: root.path)),
            root: root.path)
    }

    private func setup(id: SetupID? = nil) async -> SetupModel {
        let setup = SetupModel(
            id: id ?? .project(path: root.path), root: root, engine: engine, operations: OperationRunner())
        await setup.refresh()
        return setup
    }

    private func fixture(_ change: (inout FakeEngineService.Fixture) -> Void) {
        engine.updateFixture(root: root.path, change)
    }

    // MARK: Listing

    @Test func listsCatalogsWithKindRevisionAndLoadState() async {
        fixture { $0.sourceStates["acme/catalog"] = .notCached }
        let setup = await setup()
        let model = setup.catalogs

        await model.refresh()

        #expect(model.rows.map(\.entry.name) == ["team", "remote"])
        #expect(CatalogText.kind(model.rows[0].entry.sourceKind) == "Local folder")
        #expect(CatalogText.kind(model.rows[1].entry.sourceKind) == "GitHub")
        #expect(CatalogText.revision(model.rows[0].entry) == nil)
        #expect(CatalogText.revision(model.rows[1].entry) == "Default branch")
        #expect(model.rows[0].availability == .available(commit: nil, local: true))
        #expect(model.rows[1].availability == .notCached)
        #expect(model.hasMissing)
        #expect(engine.fixture(root: root.path).fetchCount == 0)
    }

    @Test func loadCatalogsFetchesWhatIsMissing() async {
        fixture { $0.sourceStates["acme/catalog"] = .notCached }
        let setup = await setup()
        let model = setup.catalogs
        await model.refresh()

        model.loadMissing()
        await model.waitForLoad()

        #expect(engine.fixture(root: root.path).fetchCount == 1)
        guard case .available(.some, false) = model.rows[1].availability else {
            Issue.record("remote should be loaded with a commit, got \(String(describing: model.rows[1].availability))")
            return
        }
        #expect(!model.hasMissing)
        #expect(!setup.hasPendingChanges)
    }

    @Test func failedLoadKeepsTheErrorForTheRow() async {
        let error = EngineError.network(message: "denied", detail: [], kind: .accessDenied(sso: true))
        fixture { $0.sourceStates["acme/catalog"] = .failed(error: error) }
        let setup = await setup()
        let model = setup.catalogs

        await model.refresh()

        #expect(model.rows[1].availability == .failed(error: error))
        #expect(model.canLoad)
    }

    // MARK: Adding

    @Test func addingALocalCatalogVerifiesThenStages() async throws {
        let setup = await setup()
        let model = setup.catalogs
        model.startAdding()
        let editor = try #require(model.editor)

        editor.source = "./skills-repo"

        #expect(editor.name == "skills-repo")
        #expect(editor.isLocal)
        #expect(editor.gitRef == nil)
        #expect(editor.resolvedLocalPath == root.appending(path: "skills-repo").standardized.path)
        #expect(editor.nameError == nil)
        #expect(!editor.canSave)

        editor.verify()
        await editor.waitForVerification()
        guard case .verified = editor.verification else {
            Issue.record("expected a verified source, got \(editor.verification)")
            return
        }
        #expect(editor.canSave)

        model.saveEditor()

        #expect(model.editor == nil)
        #expect(setup.hasPendingChanges)
        #expect(setup.draft?.edits == [.addCatalog(name: "skills-repo", source: "./skills-repo", gitRef: nil)])
        #expect(model.rows.last?.change == .added)
    }

    @Test func remoteSourcesOfferARevision() async throws {
        let setup = await setup()
        let model = setup.catalogs
        model.startAdding()
        let editor = try #require(model.editor)

        editor.source = "acme/skills"
        editor.revision = "v2"

        #expect(!editor.isLocal)
        #expect(editor.name == "skills")
        #expect(editor.edits == [.addCatalog(name: "skills", source: "acme/skills", gitRef: "v2")])

        editor.source = "./local"
        #expect(editor.gitRef == nil)
    }

    @Test func editingTheSourceResetsVerification() async throws {
        let setup = await setup()
        let model = setup.catalogs
        model.startAdding()
        let editor = try #require(model.editor)
        editor.source = "./one"
        editor.verify()
        await editor.waitForVerification()
        #expect(editor.canSave)

        editor.source = "./two"

        #expect(editor.verification == .idle)
        #expect(!editor.canSave)
    }

    @Test func namesAreValidatedIncludingUniqueness() async throws {
        let setup = await setup()
        let model = setup.catalogs
        model.startAdding()
        let editor = try #require(model.editor)
        editor.source = "./elsewhere"

        editor.name = "team"
        #expect(editor.nameError != nil)
        #expect(!editor.canVerify)

        editor.name = "a/b"
        #expect(editor.nameError != nil)

        editor.name = ""
        #expect(editor.nameError != nil)

        editor.name = "elsewhere"
        #expect(editor.nameError == nil)
        #expect(editor.canVerify)
    }

    @Test func aFailedVerificationBlocksStaging() async throws {
        let error = EngineError.network(message: "nope", detail: [], kind: .notFound)
        fixture { $0.sourceStates["acme/private"] = .failed(error: error) }
        let setup = await setup()
        let model = setup.catalogs
        model.startAdding()
        let editor = try #require(model.editor)
        editor.source = "acme/private"

        editor.verify()
        await editor.waitForVerification()

        #expect(editor.verification == .failed(error))
        #expect(!editor.canSave)
        model.saveEditor()
        #expect(model.editor != nil)
        #expect(!setup.hasPendingChanges)
    }

    @Test func verificationCanBeCanceled() async throws {
        fixture { $0.fetchDelay = .seconds(30) }
        let setup = await setup()
        let model = setup.catalogs
        model.startAdding()
        let editor = try #require(model.editor)
        editor.source = "acme/slow"

        editor.verify()
        #expect(editor.isVerifying)
        editor.cancelVerification()
        await editor.waitForVerification()

        #expect(editor.verification == .idle)
        await model.cancelEditor()
        #expect(model.editor == nil)
        #expect(!setup.hasPendingChanges)
        #expect(setup.draft == nil)
    }

    @Test func cancelingTheEditorChangesNothing() async throws {
        let setup = await setup()
        let model = setup.catalogs
        model.startEditing("team")
        let editor = try #require(model.editor)
        editor.source = "./other"
        editor.name = "renamed"

        await model.cancelEditor()

        #expect(model.editor == nil)
        #expect(setup.draft == nil)
        #expect(model.rows.map(\.entry.name) == ["team", "remote"])
    }

    @Test func pickedFoldersInsideAProjectAreRelative() async throws {
        let setup = await setup()
        let folder = root.appending(path: "catalogs/mine", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let outside = FileManager.default.temporaryDirectory.resolvingSymlinksInPath().path

        #expect(setup.catalogSource(forFolder: folder) == "./catalogs/mine")
        #expect(setup.catalogSource(forFolder: URL(fileURLWithPath: outside)) == outside)

        let personal = await self.setup(id: .personal)
        #expect(personal.catalogSource(forFolder: folder) == folder.resolvingSymlinksInPath().path)
    }

    // MARK: Editing

    @Test func renamingRewritesReferencesWithoutVerifying() async throws {
        let setup = await setup()
        let model = setup.catalogs
        model.startEditing("team")
        let editor = try #require(model.editor)

        editor.name = "core"

        #expect(editor.isRenaming)
        #expect(!editor.sourceChanged)
        #expect(editor.canSave)
        model.saveEditor()

        #expect(setup.draft?.edits == [.renameCatalog(from: "team", to: "core")])
        let summary = try #require(setup.configSummary)
        #expect(summary.catalogs.map(\.name) == ["core", "remote"])
        #expect(summary.requires.filter { $0.catalog == "core" }.map(\.pattern) == ["review", "lint-*"])
        #expect(!summary.requires.contains { $0.catalog == "team" })
    }

    @Test func renamingToATakenNameIsRefused() async throws {
        let setup = await setup()
        let model = setup.catalogs
        model.startEditing("team")
        let editor = try #require(model.editor)

        editor.name = "remote"

        #expect(editor.nameError != nil)
        #expect(!editor.canSave)
    }

    @Test func changingTheSourceKeepsSelectionsAndShowsUnresolvedOnes() async throws {
        fixture { $0.missingItems["./other"] = ["review"] }
        let setup = await setup()
        let model = setup.catalogs
        model.startEditing("team")
        let editor = try #require(model.editor)

        editor.source = "./other"
        #expect(!editor.canSave)
        editor.verify()
        await editor.waitForVerification()

        guard case let .verified(probe) = editor.verification else {
            Issue.record("expected a verified source, got \(editor.verification)")
            return
        }
        #expect(probe.unmatched.map(\.entry.pattern) == ["review"])
        #expect(editor.canSave)

        model.saveEditor()
        await model.refresh()

        #expect(setup.draft?.edits == [.setCatalogSource(name: "team", source: "./other", gitRef: nil)])
        #expect(setup.configSummary?.requires.contains { $0.catalog == "team" && $0.pattern == "review" } == true)
        #expect(model.unmatched.map(\.entry.pattern) == ["review"])
        #expect(model.rows.first?.change == .changed)

        model.removeUnmatched(try #require(model.unmatched.first).entry)
        await model.refresh()
        #expect(model.unmatched.isEmpty)
    }

    // MARK: Removing

    @Test func removalListsReferencesAndCancelChangesNothing() async throws {
        let setup = await setup()
        let model = setup.catalogs

        model.startRemoving("team")

        let removal = try #require(model.removal)
        #expect(removal.selections.map(\.pattern) == ["review"])
        #expect(removal.rules.map(\.pattern) == ["lint-*"])
        #expect(setup.draft == nil)

        model.cancelRemoval()

        #expect(model.removal == nil)
        #expect(setup.draft == nil)
        #expect(!setup.hasPendingChanges)
    }

    @Test func confirmingRemovalStagesTheCatalogAndItsEntriesTogether() async throws {
        let setup = await setup()
        let model = setup.catalogs
        model.startRemoving("team")

        model.confirmRemoval()

        #expect(model.removal == nil)
        #expect(setup.draft?.edits == [.removeCatalog(name: "team")])
        let summary = try #require(setup.configSummary)
        #expect(summary.catalogs.map(\.name) == ["remote"])
        #expect(summary.requires.map(\.catalog) == ["remote"])
    }

    @Test func theAreaModelIsKeptPerSetup() async {
        let setup = await setup()

        #expect(setup.catalogs === setup.catalogs)
    }
}
