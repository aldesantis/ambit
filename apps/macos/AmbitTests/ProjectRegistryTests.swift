import Foundation
import Testing

@testable import Ambit

@MainActor
struct ProjectRegistryTests {
    /// A temp dir with `home/`, `work/` and `state/`, removed when the test ends.
    @MainActor
    final class Sandbox {
        let root: URL
        let home: URL
        let work: URL
        let state: URL

        init() throws {
            root = FileManager.default.temporaryDirectory
                .appending(path: "ambit-registry-\(UUID().uuidString)", directoryHint: .isDirectory)
            home = root.appending(path: "home", directoryHint: .isDirectory)
            work = root.appending(path: "work", directoryHint: .isDirectory)
            state = root.appending(path: "state", directoryHint: .isDirectory)
            for url in [home, work] {
                try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            }
        }

        deinit {
            try? FileManager.default.removeItem(at: root)
        }

        func folder(_ name: String) throws -> URL {
            let url = work.appending(path: name, directoryHint: .isDirectory)
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
            return url
        }

        func registry() -> ProjectRegistry {
            ProjectRegistry(store: AppStateStore(directory: state), engine: FakeEngineService(), home: home)
        }
    }

    @Test func addsAProjectByCanonicalPath() async throws {
        let sandbox = try Sandbox()
        let registry = sandbox.registry()
        let folder = try sandbox.folder("app")

        let outcome = try await registry.add(folder)

        let canonical = try #require(realpathString(folder.path))
        #expect(outcome == .added(.project(path: canonical)))
        #expect(registry.projects.map(\.path) == [canonical])
        #expect(registry.projects.first?.name == "app")
    }

    @Test func dedupesSymlinksTrailingSlashesAndDotDot() async throws {
        let sandbox = try Sandbox()
        let registry = sandbox.registry()
        let folder = try sandbox.folder("app")
        let link = sandbox.work.appending(path: "link")
        try FileManager.default.createSymbolicLink(at: link, withDestinationURL: folder)

        _ = try await registry.add(folder)
        let viaLink = try await registry.add(link)
        let viaSlash = try await registry.add(URL(fileURLWithPath: folder.path + "/"))
        let viaDotDot = try await registry.add(folder.appending(path: "../app"))

        let id = SetupID.project(path: try #require(realpathString(folder.path)))
        #expect(viaLink == .existing(id))
        #expect(viaSlash == .existing(id))
        #expect(viaDotDot == .existing(id))
        #expect(registry.projects.count == 1)
    }

    @Test func homeFolderIsThePersonalSetup() async throws {
        let sandbox = try Sandbox()
        let registry = sandbox.registry()

        let outcome = try await registry.add(sandbox.home)

        #expect(outcome == .personal)
        #expect(registry.projects.isEmpty)
    }

    @Test func refusesAMissingFolder() async throws {
        let sandbox = try Sandbox()
        let registry = sandbox.registry()

        await #expect(throws: EngineError.self) {
            try await registry.add(sandbox.work.appending(path: "nope"))
        }
        #expect(registry.projects.isEmpty)
    }

    @Test func forgetRemovesOnlyTheEntry() async throws {
        let sandbox = try Sandbox()
        let registry = sandbox.registry()
        let folder = try sandbox.folder("app")
        let other = try sandbox.folder("other")
        let marker = folder.appending(path: "ambit.yaml")
        try Data("version: 1\n".utf8).write(to: marker)

        guard case let .added(id) = try await registry.add(folder), case let .project(path) = id else {
            Issue.record("Expected a new project")
            return
        }
        _ = try await registry.add(other)
        try registry.setLastActiveSetup(id)

        try registry.forget(path)

        #expect(registry.projects.map(\.name) == ["other"])
        #expect(registry.lastActiveSetup == .personal)
        #expect(FileManager.default.fileExists(atPath: marker.path))
    }

    @Test func reportsAMissingFolderAndRelocatesIt() async throws {
        let sandbox = try Sandbox()
        let registry = sandbox.registry()
        let folder = try sandbox.folder("app")
        _ = try await registry.add(try sandbox.folder("first"))
        guard case let .added(.project(path)) = try await registry.add(folder) else {
            Issue.record("Expected a new project")
            return
        }
        try registry.setLastActiveSetup(.project(path: path))
        #expect(registry.isAvailable(path))

        try FileManager.default.removeItem(at: folder)
        #expect(!registry.isAvailable(path))
        #expect(registry.projects.count == 2)

        let moved = try sandbox.folder("moved")
        let outcome = try await registry.relocate(path, to: moved)

        let movedPath = try #require(realpathString(moved.path))
        #expect(outcome == .added(.project(path: movedPath)))
        #expect(registry.projects.map(\.name) == ["first", "moved"])
        #expect(registry.lastActiveSetup == .project(path: movedPath))
        #expect(registry.isAvailable(movedPath))
    }

    @Test func relocatingOntoAnExistingProjectMergesThem() async throws {
        let sandbox = try Sandbox()
        let registry = sandbox.registry()
        let kept = try sandbox.folder("kept")
        guard case let .added(.project(gone)) = try await registry.add(try sandbox.folder("gone")) else {
            Issue.record("Expected a new project")
            return
        }
        _ = try await registry.add(kept)

        let outcome = try await registry.relocate(gone, to: kept)

        #expect(outcome == .existing(.project(path: try #require(realpathString(kept.path)))))
        #expect(registry.projects.map(\.name) == ["kept"])
    }

    @Test func persistsAcrossLaunches() async throws {
        let sandbox = try Sandbox()
        let folder = try sandbox.folder("app")
        let first = sandbox.registry()
        guard case let .added(id) = try await first.add(folder) else {
            Issue.record("Expected a new project")
            return
        }
        try first.setLastActiveSetup(id)

        let second = sandbox.registry()

        #expect(second.projects == first.projects)
        #expect(second.lastActiveSetup == id)
    }

    @Test func startsFreshFromACorruptStateFile() throws {
        let sandbox = try Sandbox()
        try FileManager.default.createDirectory(at: sandbox.state, withIntermediateDirectories: true)
        let file = sandbox.state.appending(path: AppStateStore.fileName)
        try Data("{not json".utf8).write(to: file)

        let registry = sandbox.registry()

        #expect(registry.projects.isEmpty)
        #expect(FileManager.default.fileExists(atPath: file.path + ".corrupt"))
    }

    @Test func ignoresUnknownAndMissingFields() throws {
        let sandbox = try Sandbox()
        try FileManager.default.createDirectory(at: sandbox.state, withIntermediateDirectories: true)
        let json = #"{"version": 9, "future": true, "projects": [{"path": "/x", "addedAt": "2026-01-01T00:00:00Z"}]}"#
        try Data(json.utf8).write(to: sandbox.state.appending(path: AppStateStore.fileName))

        let registry = sandbox.registry()

        #expect(registry.projects.map(\.path) == ["/x"])
        #expect(registry.lastActiveSetup == nil)
    }
}

private func realpathString(_ path: String) -> String? {
    guard let resolved = realpath(path, nil) else {
        return nil
    }
    defer { free(resolved) }
    return String(cString: resolved)
}
