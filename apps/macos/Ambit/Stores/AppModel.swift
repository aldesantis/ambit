// App-wide state behind the sidebar: the remembered projects, the selected setup, and one
// `SetupModel` per setup, created on first selection and kept while the app runs so drafts
// survive switching between setups.
//
// Leaving a setup with unapplied edits goes through `resolvePendingChanges(for:)`, which asks
// Apply / Discard / Cancel through `pendingPrompt`. The `request…` methods are the guarded entry
// points for the UI; `select` and the registry calls below them do not ask.

import Foundation
import Observation

@MainActor
@Observable
final class AppModel {
    /// What the user is about to do that would leave unapplied edits behind.
    enum PendingChangeReason: Equatable, Sendable {
        case switchSetup
        case forgetProject(path: String)
        case closeWindow
        case quit
        case restartToUpdate
    }

    enum PendingChangeChoice: Equatable, Sendable {
        case apply
        case discard
        case cancel
    }

    /// An Apply / Discard / Cancel question for one setup. Answer it with `answerPendingPrompt`.
    struct PendingChangesPrompt: Identifiable {
        let id = UUID()
        let setupName: String
        let reason: PendingChangeReason
        fileprivate let continuation: CheckedContinuation<PendingChangeChoice, Never>
    }

    let environment: AppEnvironment
    @ObservationIgnored let registry: ProjectRegistry
    let account: GitHubAccountModel
    /// Serializes every fetching or writing engine operation in the app.
    let operations = OperationRunner()

    private(set) var projects: [RememberedProject]
    private(set) var selection: SetupID?
    /// A failure to show in an alert. Cleared when the alert is dismissed.
    var presentedError: PresentedError?
    private(set) var pendingPrompt: PendingChangesPrompt?

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
        let model = SetupModel(id: id, root: root, engine: environment.engine, operations: operations)
        setupModels[id] = model
        return model
    }

    /// The setups that have unapplied edits, Personal first, then projects in sidebar order.
    var setupsWithPendingChanges: [SetupModel] {
        let order = [SetupID.personal] + projects.map { SetupID.project(path: $0.path) }
        return order.compactMap { setupModels[$0] }.filter(\.hasPendingChanges)
    }

    /// Selects `id` without asking about pending changes.
    func select(_ id: SetupID?) {
        selection = id
        guard let id else {
            return
        }

        record { try registry.setLastActiveSetup(id) }
    }

    /// Selects `id` after resolving the current setup's pending changes.
    func requestSelection(_ id: SetupID?) async {
        guard id != selection else {
            return
        }

        if await resolvePendingChanges(for: .switchSetup) {
            select(id)
        }
    }

    /// Resolves unapplied edits before `reason` proceeds, asking Apply / Discard / Cancel for each
    /// affected setup in turn. Apply runs the review sheet and continues only after a full
    /// install. Returns false when the user canceled or Apply did not install; the caller must
    /// then stop and leave the user where they are.
    ///
    /// The prompt and the review sheet are shown in the main window, so a caller acting while
    /// the window is closed (Quit from the menu bar, restart to update) must open it first.
    /// The affected setup is selected before its prompt so its review sheet can appear.
    func resolvePendingChanges(for reason: PendingChangeReason) async -> Bool {
        for setup in affectedSetups(for: reason) where setup.hasPendingChanges {
            if selection != setup.id {
                selection = setup.id
            }

            let choice = await withCheckedContinuation { continuation in
                pendingPrompt?.continuation.resume(returning: .cancel)
                pendingPrompt = PendingChangesPrompt(
                    setupName: setup.displayName, reason: reason, continuation: continuation)
            }

            switch choice {
            case .cancel:
                return false
            case .discard:
                setup.discardChanges()
            case .apply:
                guard await setup.reviewAndApply() else {
                    return false
                }
            }
        }
        return true
    }

    func answerPendingPrompt(_ choice: PendingChangeChoice) {
        guard let prompt = pendingPrompt else {
            return
        }

        pendingPrompt = nil
        prompt.continuation.resume(returning: choice)
    }

    private func affectedSetups(for reason: PendingChangeReason) -> [SetupModel] {
        switch reason {
        case .switchSetup:
            selection.flatMap { setupModels[$0] }.map { [$0] } ?? []
        case let .forgetProject(path):
            setupModels[.project(path: path)].map { [$0] } ?? []
        case .closeWindow, .quit, .restartToUpdate:
            setupsWithPendingChanges
        }
    }

    // MARK: Projects

    func isAvailable(_ project: RememberedProject) -> Bool {
        registry.isAvailable(project.path)
    }

    func project(at path: String) -> RememberedProject? {
        projects.first { $0.path == path }
    }

    /// Asks for a folder and opens it: a new project, an already remembered one, or Personal
    /// setup for the home folder.
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

    /// Forgets the project after resolving its pending changes. Nothing in its folder changes.
    /// Returns false when the user canceled.
    @discardableResult
    func forget(_ project: RememberedProject) async -> Bool {
        guard await resolvePendingChanges(for: .forgetProject(path: project.path)) else {
            return false
        }

        let id = SetupID.project(path: project.path)
        record {
            try registry.forget(project.path)
        }
        setupModels[id] = nil
        projects = registry.projects
        if selection == id {
            select(.personal)
        }
        return true
    }

    private func perform(_ failure: String, _ action: () async throws -> ProjectRegistry.Outcome) async {
        let outcome: ProjectRegistry.Outcome
        do {
            outcome = try await action()
        } catch {
            presentedError = PresentedError(title: failure, error: error)
            return
        }

        projects = registry.projects
        guard outcome.setup != selection else {
            return
        }

        if await resolvePendingChanges(for: .switchSetup) {
            select(outcome.setup)
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
