import XCTest

@MainActor
final class GitHubSignInUITests: XCTestCase {
    private let sandbox = FileManager.default.temporaryDirectory
        .appending(path: "ambit-ui-\(UUID().uuidString)", directoryHint: .isDirectory)

    override func setUpWithError() throws {
        continueAfterFailure = false
        for name in ["home", "support"] {
            try FileManager.default.createDirectory(
                at: sandbox.appending(path: name), withIntermediateDirectories: true)
        }
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: sandbox)
    }

    private func element(_ key: String, in app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == %@ OR label == %@", key, key))
            .firstMatch
    }

    func testSignInWithTheDeviceFlowAndSignOut() throws {
        let app = XCUIApplication()
        app.launchEnvironment["AMBIT_UI_TESTING"] = "1"
        app.launchEnvironment["AMBIT_TEST_HOME"] = sandbox.appending(path: "home").path
        app.launchEnvironment["AMBIT_TEST_APP_SUPPORT"] = sandbox.appending(path: "support").path
        app.launchEnvironment["AMBIT_TEST_GITHUB"] = "success"
        app.launchEnvironment["AMBIT_TEST_KEYCHAIN"] = "memory"
        app.launch()

        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 10))
        app.typeKey(",", modifierFlags: .command)
        let accountTab = app.toolbars.buttons["Account"]
        XCTAssertTrue(accountTab.waitForExistence(timeout: 5))
        accountTab.click()

        let signIn = element("account.signIn", in: app)
        XCTAssertTrue(signIn.waitForExistence(timeout: 5))
        signIn.click()

        // The fake GitHub returns this code and authorizes on its second poll.
        let code = element("signIn.code", in: app)
        XCTAssertTrue(code.waitForExistence(timeout: 5))
        XCTAssertEqual(code.value as? String ?? code.label, "AMBT-1234")
        XCTAssertTrue(element("signIn.openGitHub", in: app).exists)

        let username = element("account.username", in: app)
        XCTAssertTrue(username.waitForExistence(timeout: 10))
        XCTAssertEqual(username.value as? String ?? username.label, "octocat")
        XCTAssertFalse(code.exists)

        element("account.signOut", in: app).click()
        let confirm = element("account.confirmSignOut", in: app)
        XCTAssertTrue(confirm.waitForExistence(timeout: 5))
        confirm.click()
        XCTAssertTrue(element("account.signIn", in: app).waitForExistence(timeout: 5))
    }
}
