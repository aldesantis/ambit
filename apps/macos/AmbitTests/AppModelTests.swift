import Foundation
import Testing

@testable import Ambit

@MainActor
struct AppModelTests {
    private let sandbox: ProjectRegistryTests.Sandbox

    init() throws {
        sandbox = try ProjectRegistryTests.Sandbox()
    }

    private func model(picking folder: URL? = nil) -> AppModel {
        AppModel(
            environment: AppEnvironment(
                launch: LaunchContext(), engine: FakeEngineService(),
                stateStore: AppStateStore(directory: sandbox.state),
                folderPicker: FixedFolderPicker(folder: folder), home: sandbox.home))
    }

    @Test func startsOnThePersonalSetup() {
        let model = model()

        #expect(model.selection == .personal)
        #expect(model.setupModel(for: .personal).root == sandbox.home)
        #expect(model.setupModel(for: .personal).displayName == "Personal setup")
    }

    @Test func addingAProjectSelectsAndRemembersIt() async throws {
        let folder = try sandbox.folder("app")
        let model = model(picking: folder)

        await model.addProject()

        let project = try #require(model.projects.first)
        #expect(model.selection == .project(path: project.path))
        #expect(self.model().selection == .project(path: project.path))
    }

    @Test func addingTheHomeFolderSelectsPersonal() async {
        let model = model(picking: sandbox.home)
        model.select(nil)

        await model.addProject()

        #expect(model.projects.isEmpty)
        #expect(model.selection == .personal)
    }

    @Test func aFailedAddIsPresented() async {
        let model = model(picking: sandbox.work.appending(path: "missing"))

        await model.addProject()

        #expect(model.presentedError != nil)
        #expect(model.projects.isEmpty)
    }

    @Test func forgettingTheSelectedProjectReturnsToPersonal() async throws {
        let model = model(picking: try sandbox.folder("app"))
        await model.addProject()
        let project = try #require(model.projects.first)

        await model.forget(project)

        #expect(model.projects.isEmpty)
        #expect(model.selection == .personal)
    }

    // MARK: Pending changes

    /// A model with a remembered project and a new-setup draft in Personal setup.
    private func dirtyModel() async throws -> (AppModel, RememberedProject) {
        let model = model(picking: try sandbox.folder("app"))
        await model.addProject()
        let project = try #require(model.projects.first)
        model.select(.personal)
        let personal = model.setupModel(for: .personal)
        await personal.refresh()
        try personal.startNewSetup(harnesses: ["claude"])
        return (model, project)
    }

    @Test func switchingWithoutPendingChangesDoesNotAsk() async throws {
        let model = model(picking: try sandbox.folder("app"))
        await model.addProject()
        let project = try #require(model.projects.first)

        await model.requestSelection(.personal)
        await model.requestSelection(.project(path: project.path))

        #expect(model.pendingPrompt == nil)
        #expect(model.selection == .project(path: project.path))
    }

    @Test func cancelKeepsTheDraftAndTheSelection() async throws {
        let (model, project) = try await dirtyModel()

        let switching = Task { await model.requestSelection(.project(path: project.path)) }
        try await eventually { model.pendingPrompt != nil }
        #expect(model.pendingPrompt?.setupName == "Personal setup")
        #expect(model.pendingPrompt?.reason == .switchSetup)
        model.answerPendingPrompt(.cancel)
        await switching.value

        #expect(model.selection == .personal)
        #expect(model.setupModel(for: .personal).hasPendingChanges)
    }

    @Test func discardDropsTheDraftAndSwitches() async throws {
        let (model, project) = try await dirtyModel()

        let switching = Task { await model.requestSelection(.project(path: project.path)) }
        try await eventually { model.pendingPrompt != nil }
        model.answerPendingPrompt(.discard)
        await switching.value

        #expect(model.selection == .project(path: project.path))
        #expect(!model.setupModel(for: .personal).hasPendingChanges)
    }

    @Test func applyInstallsBeforeSwitching() async throws {
        let (model, project) = try await dirtyModel()
        let personal = model.setupModel(for: .personal)

        let switching = Task { await model.requestSelection(.project(path: project.path)) }
        try await eventually { model.pendingPrompt != nil }
        model.answerPendingPrompt(.apply)
        try await eventually {
            if case .ready = personal.review?.phase { true } else { false }
        }
        #expect(model.selection == .personal)
        await personal.review?.apply()
        await switching.value

        #expect(personal.review == nil)
        #expect(!personal.hasPendingChanges)
        #expect(personal.badge == .installed)
        #expect(model.selection == .project(path: project.path))
    }

    @Test func aFailedApplyStaysInTheSetup() async throws {
        let (model, project) = try await dirtyModel()
        let personal = model.setupModel(for: .personal)
        let engine = try #require(model.environment.engine as? FakeEngineService)
        engine.updateFixture(root: sandbox.home.path) {
            $0.applyFailure = .beforeSave(.busy(message: "Busy", detail: []))
        }

        let switching = Task { await model.requestSelection(.project(path: project.path)) }
        try await eventually { model.pendingPrompt != nil }
        model.answerPendingPrompt(.apply)
        try await eventually {
            if case .ready = personal.review?.phase { true } else { false }
        }
        await personal.review?.apply()
        personal.closeReview()
        await switching.value

        #expect(model.selection == .personal)
        #expect(personal.hasPendingChanges)
    }

    @Test func forgettingAProjectWithPendingChangesAsks() async throws {
        let model = model(picking: try sandbox.folder("app"))
        await model.addProject()
        let project = try #require(model.projects.first)
        let setup = model.setupModel(for: .project(path: project.path))
        await setup.refresh()
        try setup.startNewSetup(harnesses: ["claude"])

        let forgetting = Task { await model.forget(project) }
        try await eventually { model.pendingPrompt != nil }
        #expect(model.pendingPrompt?.reason == .forgetProject(path: project.path))
        model.answerPendingPrompt(.cancel)

        #expect(await forgetting.value == false)
        #expect(model.projects == [project])
    }

    @Test func quitAsksForEverySetupWithPendingChanges() async throws {
        let (model, project) = try await dirtyModel()
        let other = model.setupModel(for: .project(path: project.path))
        await other.refresh()
        try other.startNewSetup(harnesses: ["codex"])

        let quitting = Task { await model.resolvePendingChanges(for: .quit) }
        try await eventually { model.pendingPrompt?.setupName == "Personal setup" }
        model.answerPendingPrompt(.discard)
        try await eventually { model.pendingPrompt?.setupName == "app" }
        #expect(model.selection == .project(path: project.path))
        model.answerPendingPrompt(.discard)

        #expect(await quitting.value)
        #expect(model.setupsWithPendingChanges.isEmpty)
    }

    @Test func aForgottenLastSetupFallsBackToPersonal() throws {
        let store = AppStateStore(directory: sandbox.state)
        try store.update { $0.lastActiveSetup = .project(path: "/not/remembered") }

        #expect(model().selection == .personal)
    }
}
