// Remembered project folders. Every path is canonicalized through the engine before it is
// stored or compared, so a symlink, a trailing slash or `..` never creates a duplicate. The home
// folder is the Personal setup and is never stored as a project. A remembered folder that no
// longer exists stays in the list until the user locates or forgets it.

import Foundation

@MainActor
final class ProjectRegistry {
    enum Outcome: Equatable {
        /// The folder is the home folder.
        case personal
        case added(SetupID)
        /// The folder was already remembered.
        case existing(SetupID)

        var setup: SetupID {
            switch self {
            case .personal: .personal
            case let .added(id), let .existing(id): id
            }
        }
    }

    private let store: AppStateStore
    private let engine: any EngineService
    /// The Personal setup root as given; canonicalized on use.
    let home: URL

    init(store: AppStateStore, engine: any EngineService, home: URL) {
        self.store = store
        self.engine = engine
        self.home = home
    }

    var projects: [RememberedProject] { store.state.projects }

    var lastActiveSetup: SetupID? { store.state.lastActiveSetup }

    func setLastActiveSetup(_ setup: SetupID) throws {
        try store.update { $0.lastActiveSetup = setup }
    }

    /// Remembers `folder`, or reports it as the Personal setup or an existing project.
    ///
    /// Throws `EngineError.config` when the folder does not exist, or a Cocoa error when the
    /// state cannot be saved.
    func add(_ folder: URL) async throws -> Outcome {
        let path = try await engine.canonicalPath(folder.path)
        if await isHome(path) {
            return .personal
        }

        if projects.contains(where: { $0.path == path }) {
            return .existing(.project(path: path))
        }

        try store.update { $0.projects.append(RememberedProject(path: path, addedAt: .now)) }
        return .added(.project(path: path))
    }

    /// Points the remembered project at `path` to `folder`, keeping its place in the list. When
    /// `folder` is the home folder or another remembered project, the old entry is dropped.
    func relocate(_ path: String, to folder: URL) async throws -> Outcome {
        let newPath = try await engine.canonicalPath(folder.path)
        if await isHome(newPath) {
            try forget(path)
            return .personal
        }

        if newPath != path, projects.contains(where: { $0.path == newPath }) {
            try forget(path)
            return .existing(.project(path: newPath))
        }

        try store.update { state in
            guard let index = state.projects.firstIndex(where: { $0.path == path }) else {
                state.projects.append(RememberedProject(path: newPath, addedAt: .now))
                return
            }
            state.projects[index].path = newPath
            if state.lastActiveSetup == .project(path: path) {
                state.lastActiveSetup = .project(path: newPath)
            }
        }
        return .added(.project(path: newPath))
    }

    /// Forgets the project. Nothing in its folder is touched.
    func forget(_ path: String) throws {
        try store.update { state in
            state.projects.removeAll { $0.path == path }
            if state.lastActiveSetup == .project(path: path) {
                state.lastActiveSetup = .personal
            }
        }
    }

    /// False when the folder is gone or is no longer a directory.
    func isAvailable(_ path: String) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &isDirectory) && isDirectory.boolValue
    }

    private func isHome(_ canonical: String) async -> Bool {
        let homePath = (try? await engine.canonicalPath(home.path)) ?? home.standardizedFileURL.path
        return canonical == homePath
    }
}
