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
    /// App updates. Tests get a fake that never touches the network.
    var updater: any AppUpdater = FakeAppUpdater()
    /// The system login item. Tests get an in-memory one so real login items stay untouched.
    var loginItems: any LoginItemControl = InMemoryLoginItemControl()

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

        // Replaced by LiveEngineService once the UniFFI engine is linked.
        let engine: any EngineService = FakeEngineService()

        let isTesting = launch.isUnitTestHost || launch.isUITesting
        let updater: any AppUpdater =
            isTesting ? FakeAppUpdater(named: hooks?.updaterScenario) : SparkleAppUpdater()
        let loginItems: any LoginItemControl = isTesting ? InMemoryLoginItemControl() : SystemLoginItemControl()

        return AppEnvironment(
            launch: launch, engine: engine, stateStore: AppStateStore(directory: stateDirectory),
            folderPicker: folderPicker, home: home, updater: updater, loginItems: loginItems)
    }
}
