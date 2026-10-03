// App-wide state behind the sidebar: the remembered projects, the selected setup, and one
// `SetupModel` per setup, created on first selection and kept while the app runs.

import Foundation
import Observation

@MainActor
@Observable
final class AppModel {
    let environment: AppEnvironment
    @ObservationIgnored let registry: ProjectRegistry
    let account: GitHubAccountModel

    private(set) var projects: [RememberedProject]
    private(set) var selection: SetupID?
    /// A failure to show in an alert. Cleared when the alert is dismissed.
    var presentedError: PresentedError?

    @ObservationIgnored private var setupModels: [SetupID: SetupModel] = [:]

    init(environment: AppEnvironment) {
        self.environment = environment
        registry = ProjectRegistry(store: environment.stateStore, engine: environment.engine, home: environment.home)
        projects = registry.projects
        account = GitHubAccountModel(services: environment.gitHub, engine: environment.engine)

        let last = registry.lastActiveSetup
        if case let .project(path) = last, !registry.projects.contains(where: { $0.path == path }) {
            selection = .personal
        } else {
            selection = last ?? .personal
        }

        Task { [account] in await account.restore() }
    }

    func setupModel(for id: SetupID) -> SetupModel {
        if let model = setupModels[id] {
            return model
        }

        let root: URL =
            switch id {
            case .personal: environment.home
            case let .project(path): URL(fileURLWithPath: path, isDirectory: true)
            }
        let model = SetupModel(id: id, root: root, engine: environment.engine)
        setupModels[id] = model
        return model
    }

    func select(_ id: SetupID?) {
        selection = id
        guard let id else {
            return
        }

        record { try registry.setLastActiveSetup(id) }
    }

    func isAvailable(_ project: RememberedProject) -> Bool {
        registry.isAvailable(project.path)
    }

    func project(at path: String) -> RememberedProject? {
        projects.first { $0.path == path }
    }

    func addProject() async {
        let folder = await environment.folderPicker.pickFolder(
            title: String(localized: "Add Project"), prompt: String(localized: "Add Project"), startingAt: nil)
        guard let folder else {
            return
        }

        await perform(String(localized: "Could not add the project.")) {
            try await self.registry.add(folder)
        }
    }

    func locate(_ project: RememberedProject) async {
        let folder = await environment.folderPicker.pickFolder(
            title: String(localized: "Locate \(project.name)"), prompt: String(localized: "Use Folder"),
            startingAt: URL(fileURLWithPath: project.path).deletingLastPathComponent())
        guard let folder else {
            return
        }

        setupModels[.project(path: project.path)] = nil
        await perform(String(localized: "Could not use that folder.")) {
            try await self.registry.relocate(project.path, to: folder)
        }
    }

    func forget(_ project: RememberedProject) {
        let id = SetupID.project(path: project.path)
        record {
            try registry.forget(project.path)
        }
        setupModels[id] = nil
        projects = registry.projects
        if selection == id {
            select(.personal)
        }
    }

    private func perform(_ failure: String, _ action: () async throws -> ProjectRegistry.Outcome) async {
        do {
            let outcome = try await action()
            projects = registry.projects
            select(outcome.setup)
        } catch {
            presentedError = PresentedError(title: failure, error: error)
        }
    }

    private func record(_ action: () throws -> Void) {
        do {
            try action()
        } catch {
            presentedError = PresentedError(title: String(localized: "Could not save Ambit's settings."), error: error)
        }
    }
}

struct PresentedError: Identifiable, Equatable {
    let id = UUID()
    var title: String
    var message: String

    init(title: String, error: any Error) {
        self.title = title
        message = error.localizedDescription
    }
}
