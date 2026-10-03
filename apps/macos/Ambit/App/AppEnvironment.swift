// The services the app runs with, chosen once at launch from the `LaunchContext`. Stores receive
// them from here and never construct services themselves, so tests swap any of them.

import Foundation

@MainActor
struct AppEnvironment {
    var launch: LaunchContext
    var engine: any EngineService
    var stateStore: AppStateStore
    var folderPicker: any FolderPicker
    /// The Personal setup root and the engine's HOME.
    var home: URL
    var gitHub: GitHubServices = .inert

    static func make(for launch: LaunchContext) -> AppEnvironment {
        let hooks = launch.testHooks
        let home = hooks?.home ?? FileManager.default.homeDirectoryForCurrentUser

        let stateDirectory: URL
        if let appSupport = hooks?.appSupport {
            stateDirectory = appSupport
        } else if launch.isUnitTestHost || launch.isUITesting {
            stateDirectory = FileManager.default.temporaryDirectory
                .appending(path: "ambit-test-\(ProcessInfo.processInfo.processIdentifier)", directoryHint: .isDirectory)
        } else {
            stateDirectory = AppStateStore.defaultDirectory
        }

        let folderPicker: any FolderPicker =
            if let hooks { FixedFolderPicker(folder: hooks.pickFolder) } else { OpenPanelFolderPicker() }

        // UI tests run a seeded fake engine (`AMBIT_TEST_ENGINE=<scenario>`), or the real one on the
        // test home with `AMBIT_TEST_ENGINE=live`.
        let engine: any EngineService =
            if let hooks, hooks.engineScenario != "live" {
                FakeEngineService.scenario(hooks.engineScenario, home: home)
            } else {
                // Under UI testing every location derives from the test home, never the developer's.
                LiveEngineService(
                    environment: LiveEngineEnvironment.make(home: home, forwardProcess: hooks == nil))
            }

        return AppEnvironment(
            launch: launch, engine: engine, stateStore: AppStateStore(directory: stateDirectory),
            folderPicker: folderPicker, home: home, gitHub: .make(for: launch))
    }
}
