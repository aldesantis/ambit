import AmbitEngine
import Foundation
import Testing

@testable import Ambit

// The app's types win over the bindings' same-named ones; FFI types are spelled `AmbitEngine.X`.
private typealias EngineError = Ambit.EngineError
private typealias ProgressEvent = Ambit.ProgressEvent
private typealias SelectionEntry = Ambit.SelectionEntry
private typealias ItemRef = Ambit.ItemRef
private typealias CatalogLoadState = Ambit.CatalogLoadState

/// The Rust engine through the facade, on temp dirs with their own HOME.
@MainActor
struct LiveEngineServiceTests {
    /// A temp dir with `home/` and `work/`, removed when the test ends.
    final class Sandbox {
        let root: URL
        let home: URL
        let work: URL

        init() throws {
            root = FileManager.default.temporaryDirectory
                .appending(path: "ambit-live-\(UUID().uuidString)", directoryHint: .isDirectory)
            home = root.appending(path: "home", directoryHint: .isDirectory)
            work = root.appending(path: "work", directoryHint: .isDirectory)
            for url in [home, work] {
                try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            }
        }

        deinit {
            try? FileManager.default.removeItem(at: root)
        }

        func engine(git: URL? = LiveEngineEnvironment.bundledGit()) -> LiveEngineService {
            LiveEngineService(environment: LiveEngineEnvironment.make(home: home, forwardProcess: false, git: git))
        }

        func folder(_ name: String, files: [String: String] = [:]) throws -> URL {
            let url = work.appending(path: name, directoryHint: .isDirectory)
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            for (file, text) in files {
                try text.write(to: url.appending(path: file), atomically: true, encoding: .utf8)
            }
            return url
        }
    }

    // MARK: Selection

    @Test func theAppRunsTheLiveEngineExceptUnderUITests() {
        let live = AppEnvironment.make(for: LaunchContext(isUnitTestHost: true))
        let uiFake = AppEnvironment.make(
            for: LaunchContext(environment: ["AMBIT_UI_TESTING": "1"], arguments: []))
        let uiScenario = AppEnvironment.make(
            for: LaunchContext(
                environment: ["AMBIT_UI_TESTING": "1", "AMBIT_TEST_ENGINE": "capabilities"], arguments: []))
        let uiLive = AppEnvironment.make(
            for: LaunchContext(environment: ["AMBIT_UI_TESTING": "1", "AMBIT_TEST_ENGINE": "live"], arguments: []))
        let ignored = AppEnvironment.make(
            for: LaunchContext(environment: ["AMBIT_TEST_ENGINE": "capabilities"], arguments: []))

        #expect(live.engine is LiveEngineService)
        #expect(uiFake.engine is FakeEngineService)
        #expect(uiScenario.engine is FakeEngineService)
        #expect(uiLive.engine is LiveEngineService)
        #expect(ignored.engine is LiveEngineService)
    }

    // MARK: Snapshot

    @Test func snapshotOfAFolderWithoutConfigIsMissing() async throws {
        let sandbox = try Sandbox()
        let root = try sandbox.folder("empty")

        let snapshot = try await sandbox.engine().openSetup(root: root.path).snapshot()

        #expect(snapshot.root == root.path)
        #expect(snapshot.config == .missing)
    }

    @Test func snapshotReadsAValidConfig() async throws {
        let sandbox = try Sandbox()
        let text = """
            version: 1
            harnesses: [claude, cursor]
            catalogs:
              - name: company
                source: acme/skills
            requires:
              - skill: company/core.*
            """
        let root = try sandbox.folder("valid", files: ["ambit.yml": text])

        let snapshot = try await sandbox.engine().openSetup(root: root.path).snapshot()

        guard case let .valid(path, fileName, readText, summary) = snapshot.config else {
            Issue.record("expected a valid config, got \(snapshot.config)")
            return
        }
        #expect(path.hasSuffix("/valid/ambit.yml"))
        #expect(fileName == "ambit.yml")
        #expect(readText == text)
        #expect(summary.harnesses == ["claude", "cursor"])
        #expect(summary.catalogs.map(\.name) == ["company"])
        #expect(
            summary.catalogs.first?.sourceKind
                == .git(url: "https://github.com/acme/skills.git", github: true, commitRef: false))
        #expect(summary.requires == [SelectionEntry(kind: .skill, catalog: "company", pattern: "core.*", isRule: true)])
    }

    @Test func snapshotReportsAnInvalidConfigWithItsLine() async throws {
        let sandbox = try Sandbox()
        let root = try sandbox.folder("invalid", files: ["ambit.yml": "version: 1\nextra: true\n"])

        let snapshot = try await sandbox.engine().openSetup(root: root.path).snapshot()

        guard case let .invalid(_, fileName, problem) = snapshot.config else {
            Issue.record("expected an invalid config, got \(snapshot.config)")
            return
        }
        #expect(fileName == "ambit.yml")
        #expect(problem.line == 2)
        #expect(!problem.message.isEmpty)
    }

    @Test func snapshotReportsTwoConfigFilesAsAmbiguous() async throws {
        let sandbox = try Sandbox()
        let root = try sandbox.folder(
            "ambiguous", files: ["ambit.yml": "version: 1\n", "ambit.yaml": "version: 1\n"])

        let snapshot = try await sandbox.engine().openSetup(root: root.path).snapshot()

        guard case let .ambiguous(files, problem) = snapshot.config else {
            Issue.record("expected an ambiguous config, got \(snapshot.config)")
            return
        }
        #expect(Set(files) == ["ambit.yml", "ambit.yaml"])
        #expect(!problem.message.isEmpty)
    }

    // MARK: Canonical paths

    @Test func canonicalPathResolvesSymlinksAndDotDot() async throws {
        let sandbox = try Sandbox()
        let engine = sandbox.engine()
        let folder = try sandbox.folder("app")
        let link = sandbox.work.appending(path: "link")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: folder)

        let direct = try await engine.canonicalPath(folder.path)

        #expect(try await engine.canonicalPath(link.path) == direct)
        #expect(try await engine.canonicalPath(folder.path + "/") == direct)
        #expect(try await engine.canonicalPath(folder.appending(path: "../app").path) == direct)
    }

    @Test func canonicalPathOfAMissingFolderIsAnIOError() async throws {
        let sandbox = try Sandbox()

        await #expect {
            _ = try await sandbox.engine().canonicalPath(sandbox.work.appending(path: "nope").path)
        } throws: { error in
            guard case .io = error as? EngineError else { return false }
            return true
        }
    }

    @Test func registryDedupesThroughTheLiveEngine() async throws {
        let sandbox = try Sandbox()
        let state = sandbox.root.appending(path: "state", directoryHint: .isDirectory)
        let registry = ProjectRegistry(
            store: AppStateStore(directory: state), engine: sandbox.engine(), home: sandbox.home)
        let folder = try sandbox.folder("app")
        let link = sandbox.work.appending(path: "link")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: folder)

        let added = try await registry.add(folder)
        let again = try await registry.add(link)

        #expect(again == .existing(added.setup))
        #expect(registry.projects.count == 1)
        #expect(try await registry.add(sandbox.home) == .personal)
    }

    // MARK: Pure functions

    @Test func describesSourcesAndRejectsGarbage() throws {
        let engine = try Sandbox().engine()

        let info = try engine.describeSource("acme/skills", gitRef: nil)

        #expect(info.proposedName == "skills")
        #expect(info.kind == .git(url: "https://github.com/acme/skills.git", github: true, commitRef: false))
        #expect {
            _ = try engine.describeSource("not a source", gitRef: nil)
        } throws: { error in
            guard case .config = error as? EngineError else { return false }
            return true
        }
    }

    @Test func listsTheFiveAgentTools() throws {
        let tools = try Sandbox().engine().supportedAgentTools()

        #expect(tools.map(\.id) == ["claude", "codex", "cursor", "opencode", "vscode"])
        #expect(tools.first?.displayName == "Claude Code")
    }

    @Test func editsTheConfigAndReportsTheChanges() throws {
        let engine = try Sandbox().engine()
        let base = engine.newConfigText(harnesses: ["claude"])
        let rule = SelectionEntry(kind: .skill, catalog: "company", pattern: "core.*", isRule: true)

        let edited = try engine.editConfig(
            text: base, fileName: "ambit.yml",
            edits: [.addCatalog(name: "company", source: "acme/skills", gitRef: nil), .addEntry(rule)])

        #expect(edited.summary.catalogs.map(\.name) == ["company"])
        #expect(edited.summary.requires == [rule])
        #expect(try engine.parseConfig(text: edited.text, fileName: "ambit.yml") == edited.summary)
        #expect(try engine.catalogReferences(text: edited.text, fileName: "ambit.yml", catalog: "company") == [rule])

        let changes = try engine.configChanges(base: base, draft: edited.text, fileName: "ambit.yml")
        #expect(changes.catalogsAdded.map(\.name) == ["company"])
        #expect(changes.entriesAdded == [rule])

        #expect(throws: EngineError.self) {
            try engine.validateCatalogName(text: edited.text, fileName: "ambit.yml", name: "company", renaming: nil)
        }
        try engine.validateCatalogName(text: edited.text, fileName: "ambit.yml", name: "company", renaming: "company")
    }

    // MARK: Git

    /// A stand-in for the bundled git: an app bundle whose Resources/git/bin/git prints a version.
    private func fakeBundle(in sandbox: Sandbox) throws -> Bundle {
        let app = sandbox.root.appending(path: "Fake.app", directoryHint: .isDirectory)
        let bin = app.appending(path: "Contents/Resources/git/bin", directoryHint: .isDirectory)
        try FileManager.default.createDirectory(at: bin, withIntermediateDirectories: true)
        let git = bin.appending(path: "git")
        try "#!/bin/sh\necho 'git version 9.9.9 (bundled)'\n".write(to: git, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: git.path)
        return try #require(Bundle(url: app))
    }

    @Test func theEngineRunsTheBundledGitWhenPresent() async throws {
        let sandbox = try Sandbox()
        let bundled = try #require(LiveEngineEnvironment.bundledGit(in: fakeBundle(in: sandbox)))

        let env = LiveEngineEnvironment.make(home: sandbox.home, forwardProcess: false, git: bundled)

        #expect(env[LiveEngineEnvironment.gitProgramVariable] == bundled.path)
        #expect(env["PATH"]?.hasPrefix(bundled.deletingLastPathComponent().path + ":") == true)
        #expect(try await sandbox.engine(git: bundled).gitVersion() == "git version 9.9.9 (bundled)")
    }

    @Test func withoutABundledGitTheEngineUsesGitOnThePath() async throws {
        let sandbox = try Sandbox()

        let env = LiveEngineEnvironment.make(home: sandbox.home, forwardProcess: false, git: nil)

        #expect(env[LiveEngineEnvironment.gitProgramVariable] == nil)
        #expect(env["PATH"] == "/usr/bin:/bin:/usr/sbin:/sbin")
        #expect(try await sandbox.engine(git: nil).gitVersion().hasPrefix("git version"))
    }

    @Test func thisBuildsBundledGitRunsWhenEmbedded() async throws {
        // Present only when scripts/build-git.sh ran before the build.
        guard let bundled = LiveEngineEnvironment.bundledGit() else {
            return
        }
        let sandbox = try Sandbox()

        #expect(try await sandbox.engine(git: bundled).gitVersion().hasPrefix("git version"))
    }

    @Test func environmentForwardsOnlyWhatItShould() {
        let home = URL(fileURLWithPath: "/Users/someone")
        let process = ["XDG_CACHE_HOME": "/cache", "SSH_AUTH_SOCK": "/agent", "SECRET": "x", "HOME": "/elsewhere"]

        let forwarded = LiveEngineEnvironment.make(home: home, process: process, git: nil)
        let isolated = LiveEngineEnvironment.make(home: home, process: process, forwardProcess: false, git: nil)

        #expect(forwarded["XDG_CACHE_HOME"] == "/cache")
        #expect(forwarded["SSH_AUTH_SOCK"] == "/agent")
        #expect(forwarded["SECRET"] == nil)
        #expect(forwarded["HOME"] == "/Users/someone")
        #expect(isolated == ["HOME": "/Users/someone", "PATH": "/usr/bin:/bin:/usr/sbin:/sbin"])
    }

    // MARK: Browse

    /// A local catalog (pack `base` requiring skill `alpha`, and skill `beta`) and a project using it.
    private func browseFixture(in sandbox: Sandbox, requires: [String]) throws -> URL {
        let catalog = try sandbox.folder("catalog")
        let files = [
            "skills/alpha/SKILL.md": "---\nname: alpha\ndescription: The first skill.\n---\n\n# Alpha\n",
            "skills/beta/SKILL.md": "---\nname: beta\ndescription: |\n  The second\n  skill.\n---\n\n# Beta\n",
            "packs/base.yml": "name: base\ndescription: The basics.\nrequires:\n  - skill: alpha\n",
        ]
        for (path, text) in files {
            let url = catalog.appending(path: path)
            try FileManager.default.createDirectory(
                at: url.deletingLastPathComponent(), withIntermediateDirectories: true)
            try text.write(to: url, atomically: true, encoding: .utf8)
        }
        return try sandbox.folder("project", files: ["ambit.yml": Self.browseConfig(requires: requires)])
    }

    private static func browseConfig(requires: [String]) -> String {
        let entries = requires.isEmpty ? " []" : requires.map { "\n  - \($0)" }.joined()
        return "version: 1\nharnesses: [claude]\ncatalogs:\n  - name: company\n    source: path:../catalog\n"
            + "requires:\(entries)\n"
    }

    @Test func loadsAndBrowsesALocalCatalog() async throws {
        let sandbox = try Sandbox()
        let project = try browseFixture(in: sandbox, requires: [#"pack: "company/base""#])
        let session = sandbox.engine().openSetup(root: project.path)
        let base = ItemRef(kind: .pack, catalog: "company", name: "base")

        let state = try await session.loadCatalogs(draftText: nil, policy: .cacheOnly, progress: nil)
        let result = try await session.browse(draftText: nil)

        #expect(state.catalogs == [CatalogLoadState(name: "company", availability: .available(commit: nil, local: true))])
        #expect(result.problems.isEmpty)
        #expect(result.items.map(\.item.name) == ["base", "alpha", "beta"])
        let alpha = try #require(result.items.first { $0.item.name == "alpha" })
        #expect(alpha.selected)
        #expect(alpha.description == "The first skill.")
        #expect(alpha.routes == [.pack(pack: base, chain: [base])])
        let pack = try #require(result.items.first { $0.item == base })
        #expect(pack.routes == [.direct(entry: SelectionEntry(kind: .pack, catalog: "company", pattern: "base", isRule: false))])
        #expect(pack.detail == .pack(requires: [SelectionEntry(kind: .skill, catalog: nil, pattern: "alpha", isRule: false)]))
    }

    @Test func readsDocumentsPacksRulesAndRemovalImpact() async throws {
        let sandbox = try Sandbox()
        let project = try browseFixture(in: sandbox, requires: [])
        let session = sandbox.engine().openSetup(root: project.path)
        _ = try await session.loadCatalogs(draftText: nil, policy: .cacheOnly, progress: nil)

        let document = try await session.skillDocument(catalog: "company", name: "beta")
        #expect(document.body.contains("# Beta"))
        #expect(document.frontmatter.map(\.key) == ["name", "description"])
        #expect(document.frontmatter.last?.value == "|\n  The second\n  skill.")

        let contents = try await session.packContents(catalog: "company", name: "base")
        #expect(contents.map(\.name) == ["base", "alpha"])

        let rule = SelectionEntry(kind: .skill, catalog: "company", pattern: "*a", isRule: true)
        #expect(try await session.ruleMatches(draftText: nil, entry: rule).map(\.name) == ["alpha", "beta"])

        let draft = Self.browseConfig(requires: [#"pack: "company/base""#, #"skill: "company/alpha""#])
        let alpha = ItemRef(kind: .skill, catalog: "company", name: "alpha")
        let impact = try await session.removalImpact(draftText: draft, item: alpha)
        #expect(impact.item == alpha)
        #expect(impact.removed.map(\.name) == ["base", "alpha"])
        #expect(try await session.previewRule(draftText: nil, catalog: "company", kind: .skill, pattern: "*a").count == 2)
        #expect(try await session.unmatchedEntries(draftText: draft).isEmpty)
        let unmatched = try await session.unmatchedEntries(
            draftText: Self.browseConfig(requires: [#"skill: "company/nothing""#]))
        #expect(unmatched.map(\.entry.pattern) == ["nothing"])
        #expect(impact.sustaining.map(\.entry.pattern) == ["base", "alpha"])
        let base = ItemRef(kind: .pack, catalog: "company", name: "base")
        #expect(impact.sustaining.first?.routes == [.pack(pack: base, chain: [base])])
        #expect(impact.sustaining.last?.routes == [.direct(entry: SelectionEntry(kind: .skill, catalog: "company", pattern: "alpha", isRule: false))])
    }

    @Test func loadingCatalogsReportsProgressAndStopsWhenCanceled() async throws {
        @MainActor final class Recorder {
            var subjects: [String] = []
        }
        let sandbox = try Sandbox()
        let project = try browseFixture(in: sandbox, requires: [])
        let session = sandbox.engine().openSetup(root: project.path)
        let recorder = Recorder()

        _ = try await session.loadCatalogs(draftText: nil, policy: .cacheOnly) { recorder.subjects.append($0.subject) }
        // Progress hops through the main queue; let it drain.
        await Task.yield()
        try await Task.sleep(for: .milliseconds(50))
        #expect(recorder.subjects == ["company"])

        let canceled = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            return try await session.loadCatalogs(draftText: nil, policy: .fetchMissing, progress: nil)
        }
        await #expect(throws: EngineError.canceled) { _ = try await canceled.value }
    }

    // MARK: Errors, cancellation, progress

    @Test func mapsEveryFFIErrorCase() {
        let cases: [(AmbitEngine.EngineError, EngineError)] = [
            (.Config(message: "m", detail: ["d"], path: "ambit.yml", line: 3),
             .config(message: "m", detail: ["d"], path: "ambit.yml", line: 3)),
            (.Network(message: "m", detail: [], kind: .accessDenied(sso: true)),
             .network(message: "m", detail: [], kind: .accessDenied(sso: true))),
            (.StaleReview(message: "m", detail: []), .staleReview(message: "m", detail: [])),
            (.Busy(message: "m", detail: []), .busy(message: "m", detail: [])),
            (.Canceled, .canceled),
            (.Io(message: "m", detail: [], path: "/x"), .io(message: "m", detail: [], path: "/x")),
        ]

        for (ffi, expected) in cases {
            #expect(LiveEngineMapping.engineError(ffi) == expected)
        }
    }

    @Test func cancellingTheTaskCancelsTheEngineToken() async throws {
        let executor = LiveEngineExecutor()
        let started = AsyncStream<Void>.makeStream()

        let task = Task {
            try await executor.operation(.mutation, progress: nil) { token, _ in
                started.continuation.yield()
                // Stands in for an export that checks its token between steps.
                let deadline = Date().addingTimeInterval(10)
                while !token.isCanceled() {
                    if Date() > deadline {
                        return false
                    }
                    Thread.sleep(forTimeInterval: 0.005)
                }
                throw AmbitEngine.EngineError.Canceled
            }
        }
        var iterator = started.stream.makeAsyncIterator()
        await iterator.next()
        task.cancel()

        await #expect(throws: EngineError.canceled) { _ = try await task.value }
    }

    @Test func progressArrivesOnTheMainActorInOrder() async throws {
        @MainActor final class Recorder {
            var events: [ProgressEvent] = []
        }
        let executor = LiveEngineExecutor()
        let recorder = Recorder()
        let done = AsyncStream<Void>.makeStream()

        try await executor.operation(.read, progress: { event in
            recorder.events.append(event)
            if event.current == 2 {
                done.continuation.yield()
            }
        }) { _, listener in
            for current in UInt32(1)...2 {
                listener?.onProgress(
                    event: AmbitEngine.ProgressEvent(stage: .fetching, subject: "company", current: current, total: 2))
            }
        }
        var iterator = done.stream.makeAsyncIterator()
        await iterator.next()

        #expect(recorder.events.map(\.current) == [1, 2])
        #expect(recorder.events.allSatisfy { $0.stage == .fetching && $0.subject == "company" })
    }

    // MARK: Verify, review, apply, status, health

    @Test func verifiesACatalogWithoutTouchingTheSession() async throws {
        let sandbox = try Sandbox()
        let project = try browseFixture(in: sandbox, requires: [])
        let session = sandbox.engine().openSetup(root: project.path)

        let probe = try await session.verifyCatalog(name: "other", source: "path:../catalog", gitRef: nil, progress: nil)

        #expect(probe.name == "other")
        #expect(probe.sourceKind == .local(path: "../catalog"))
        #expect(probe.commit == nil)
        #expect(probe.counts == ItemCounts(skills: 2, packs: 1, mcps: 0, hooks: 0))
        await #expect(throws: EngineError.self) {
            _ = try await session.verifyCatalog(name: "gone", source: "path:../nowhere", gitRef: nil, progress: nil)
        }
    }

    @Test func reviewsAppliesAndReportsStatusAndHealth() async throws {
        let sandbox = try Sandbox()
        let project = try browseFixture(in: sandbox, requires: [])
        let session = sandbox.engine().openSetup(root: project.path)
        let draft = Self.browseConfig(requires: [#"skill: "company/alpha""#])

        let review = try await session.review(draftText: draft, progress: nil)

        #expect(review.summary.canApply)
        #expect(review.summary.blockers.isEmpty)
        #expect(review.summary.diff.added.map(\.name) == ["alpha"])
        #expect(review.summary.config.entriesAdded.map(\.pattern) == ["alpha"])
        #expect(!review.summary.writes.isEmpty)

        let outcome = try await session.apply(review, progress: nil)

        #expect(
            outcome
                == .installed(summary: InstallSummary(writes: review.summary.writes, removals: review.summary.removals)))
        #expect(try String(contentsOf: project.appending(path: "ambit.yml"), encoding: .utf8) == draft)

        let status = try await session.status()
        #expect(status.items.map(\.item.name) == ["alpha"])
        #expect(status.items.allSatisfy { $0.state == .ok })

        let health = try await session.health()
        #expect(!health.checks.isEmpty)
        #expect(!health.environmentNote.isEmpty)

        let retried = try await session.retryInstall(progress: nil)
        guard case .installed = retried else {
            Issue.record("expected the retry to install, got \(retried)")
            return
        }
    }

    @Test func aChangedConfigMakesTheReviewStale() async throws {
        let sandbox = try Sandbox()
        let project = try browseFixture(in: sandbox, requires: [])
        let session = sandbox.engine().openSetup(root: project.path)

        let review = try await session.review(
            draftText: Self.browseConfig(requires: [#"skill: "company/alpha""#]), progress: nil)
        try Self.browseConfig(requires: [#"skill: "company/beta""#])
            .write(to: project.appending(path: "ambit.yml"), atomically: true, encoding: .utf8)

        await #expect {
            _ = try await session.apply(review, progress: nil)
        } throws: { error in
            guard case .staleReview = error as? EngineError else { return false }
            return true
        }
    }

    @Test func aBlockedReviewSaysWhy() async throws {
        let sandbox = try Sandbox()
        let project = try browseFixture(in: sandbox, requires: [])
        let session = sandbox.engine().openSetup(root: project.path)

        let review = try await session.review(
            draftText: Self.browseConfig(requires: [#"skill: "company/nothing""#]), progress: nil)

        #expect(!review.summary.canApply)
        guard case let .unmatched(entry, _) = review.summary.blockers.first else {
            Issue.record("expected an unmatched entry, got \(review.summary.blockers)")
            return
        }
        #expect(entry.pattern == "nothing")
    }

    @Test func checksALocalCatalogForUpdates() async throws {
        let sandbox = try Sandbox()
        let project = try browseFixture(in: sandbox, requires: [])
        let session = sandbox.engine().openSetup(root: project.path)

        let check = try await session.checkCatalogUpdate(catalog: "company", progress: nil)

        #expect(check.catalog == "company")
        #expect(check.freshness == .local)
    }
}
