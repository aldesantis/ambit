// How this process was launched. The `AMBIT_TEST_*` hooks are read only when
// `AMBIT_UI_TESTING=1`, so a stray variable in a user's environment never changes behavior.

import Foundation

struct LaunchContext: Sendable, Equatable {
    /// The scenario names are owned by the fakes that read them (GitHub, updater).
    struct TestHooks: Sendable, Equatable {
        var home: URL?
        var appSupport: URL?
        var pickFolder: URL?
        var gitHubScenario: String?
        var updaterScenario: String?
        var inMemoryKeychain = false
        /// A `FakeEngineService.scenario` name.
        var engineScenario: String?
    }

    /// Non-nil only under `AMBIT_UI_TESTING=1`.
    var testHooks: TestHooks?

    /// True when the process hosts the unit test bundle. The app must then leave real user state
    /// alone and must not enforce a single instance.
    var isUnitTestHost = false

    /// Launched at login: the main window stays closed. Set from `--login-launch` under UI
    /// testing; the login-item Apple event check belongs to the lifecycle code.
    var isLoginLaunch = false

    var isUITesting: Bool { testHooks != nil }

    static var current: LaunchContext {
        LaunchContext(environment: ProcessInfo.processInfo.environment, arguments: ProcessInfo.processInfo.arguments)
    }

    init(testHooks: TestHooks? = nil, isUnitTestHost: Bool = false, isLoginLaunch: Bool = false) {
        self.testHooks = testHooks
        self.isUnitTestHost = isUnitTestHost
        self.isLoginLaunch = isLoginLaunch
    }

    init(environment: [String: String], arguments: [String]) {
        isUnitTestHost = environment["XCTestConfigurationFilePath"] != nil

        guard environment["AMBIT_UI_TESTING"] == "1" else {
            return
        }

        func url(_ name: String) -> URL? {
            guard let value = environment[name], !value.isEmpty else {
                return nil
            }
            return URL(fileURLWithPath: value, isDirectory: true)
        }

        testHooks = TestHooks(
            home: url("AMBIT_TEST_HOME"),
            appSupport: url("AMBIT_TEST_APP_SUPPORT"),
            pickFolder: url("AMBIT_TEST_PICK_FOLDER"),
            gitHubScenario: environment["AMBIT_TEST_GITHUB"],
            updaterScenario: environment["AMBIT_TEST_UPDATER"],
            inMemoryKeychain: environment["AMBIT_TEST_KEYCHAIN"] == "memory",
            engineScenario: environment["AMBIT_TEST_ENGINE"])
        isLoginLaunch = arguments.contains("--login-launch")
    }
}
