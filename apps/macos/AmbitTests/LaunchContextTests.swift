import Foundation
import Testing

@testable import Ambit

struct LaunchContextTests {
    @Test func ignoresTestHooksOutsideUITesting() {
        let context = LaunchContext(
            environment: ["AMBIT_TEST_HOME": "/tmp/home", "AMBIT_TEST_KEYCHAIN": "memory"],
            arguments: ["Ambit", "--login-launch"])

        #expect(context.testHooks == nil)
        #expect(!context.isLoginLaunch)
    }

    @Test func readsTestHooksUnderUITesting() {
        let context = LaunchContext(
            environment: [
                "AMBIT_UI_TESTING": "1",
                "AMBIT_TEST_HOME": "/tmp/home",
                "AMBIT_TEST_APP_SUPPORT": "/tmp/support",
                "AMBIT_TEST_PICK_FOLDER": "/tmp/pick",
                "AMBIT_TEST_GITHUB": "denied",
                "AMBIT_TEST_UPDATER": "staged",
                "AMBIT_TEST_KEYCHAIN": "memory",
            ],
            arguments: ["Ambit", "--login-launch"])

        let hooks = context.testHooks
        #expect(hooks?.home?.path == "/tmp/home")
        #expect(hooks?.appSupport?.path == "/tmp/support")
        #expect(hooks?.pickFolder?.path == "/tmp/pick")
        #expect(hooks?.gitHubScenario == "denied")
        #expect(hooks?.updaterScenario == "staged")
        #expect(hooks?.inMemoryKeychain == true)
        #expect(context.isLoginLaunch)
    }
}
