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

        model.forget(project)

        #expect(model.projects.isEmpty)
        #expect(model.selection == .personal)
    }

    @Test func aForgottenLastSetupFallsBackToPersonal() throws {
        let store = AppStateStore(directory: sandbox.state)
        try store.update { $0.lastActiveSetup = .project(path: "/not/remembered") }

        #expect(model().selection == .personal)
    }
}
