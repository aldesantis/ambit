import XCTest

/// Window close, reopen, login launch, the menu bar and update actions. Uses the fake updater
/// (`AMBIT_TEST_UPDATER`) and an in-memory login item.
@MainActor
final class MenuBarUITests: XCTestCase {
    private let sandbox = FileManager.default.temporaryDirectory
        .appending(path: "ambit-ui-\(UUID().uuidString)", directoryHint: .isDirectory)

    override func setUpWithError() throws {
        continueAfterFailure = false
        for name in ["home", "support"] {
            try FileManager.default.createDirectory(at: sandbox.appending(path: name), withIntermediateDirectories: true)
        }
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: sandbox)
    }

    private func launch(updater: String = "none", arguments: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["AMBIT_UI_TESTING"] = "1"
        app.launchEnvironment["AMBIT_TEST_HOME"] = sandbox.appending(path: "home").path
        app.launchEnvironment["AMBIT_TEST_APP_SUPPORT"] = sandbox.appending(path: "support").path
        app.launchEnvironment["AMBIT_TEST_UPDATER"] = updater
        app.launchArguments = arguments
        app.launch()
        return app
    }

    /// Opens the menu bar extra's menu. Skips when the status item cannot be clicked: macOS hides
    /// status items that do not fit (for example beside the camera housing), and XCUITest
    /// cannot click a hidden one.
    private func openStatusMenu(_ app: XCUIApplication) throws -> XCUIElement {
        let item = app.statusItems.firstMatch
        guard item.waitForExistence(timeout: 5) else {
            throw XCTSkip("The menu bar extra's status item is not exposed to XCUITest.")
        }
        guard item.isHittable else {
            throw XCTSkip("The status item is hidden by macOS (no room in the menu bar), so XCUITest cannot click it.")
        }
        item.click()
        let menu = item.menus.firstMatch
        XCTAssertTrue(menu.waitForExistence(timeout: 5))
        return menu
    }

    private func openUpdatesSettings(_ app: XCUIApplication) -> XCUIElement {
        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 10))
        app.typeKey(",", modifierFlags: .command)
        let tab = app.toolbars.buttons["Updates"]
        XCTAssertTrue(tab.waitForExistence(timeout: 5))
        tab.click()
        return app
    }

    func testClosingTheWindowKeepsTheAppRunning() throws {
        let app = launch()
        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 10))

        app.typeKey("w", modifierFlags: .command)

        XCTAssertTrue(app.windows.firstMatch.waitForNonExistence(timeout: 5))
        XCTAssertNotEqual(app.state, .notRunning)
    }

    func testOpenAmbitFromTheMenuReopensTheWindow() throws {
        let app = launch()
        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 10))
        app.typeKey("w", modifierFlags: .command)
        XCTAssertTrue(app.windows.firstMatch.waitForNonExistence(timeout: 5))

        let menu = try openStatusMenu(app)
        menu.menuItems["Open Ambit"].click()

        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 5))
    }

    func testLoginLaunchDoesNotOpenTheWindow() throws {
        let app = launch(arguments: ["--login-launch"])

        // Give SwiftUI time to open a window it should not open.
        XCTAssertFalse(app.windows.firstMatch.waitForExistence(timeout: 3))
        XCTAssertNotEqual(app.state, .notRunning)
    }

    func testStagedUpdateOffersRestartInSettings() throws {
        let app = openUpdatesSettings(launch(updater: "staged"))

        let status = app.staticTexts["settings.updateStatus"]
        XCTAssertTrue(status.waitForExistence(timeout: 5))
        XCTAssertEqual(status.value as? String, "Ambit 99.0 is ready to install")

        let restart = app.buttons["settings.restartToUpdate"]
        XCTAssertTrue(restart.exists)
        restart.click()

        // Nothing pending, so the fake updater is told to install.
        XCTAssertTrue(
            app.staticTexts.matching(NSPredicate(format: "value == %@", "Installing update…")).firstMatch
                .waitForExistence(timeout: 5))
    }

    func testFailedDownloadOffersRetryInSettings() throws {
        let app = openUpdatesSettings(launch(updater: "downloadFailed"))

        let retry = app.buttons["settings.checkForUpdates"]
        XCTAssertTrue(retry.waitForExistence(timeout: 5))
        XCTAssertEqual(retry.title.isEmpty ? retry.label : retry.title, "Retry")
        retry.click()

        XCTAssertTrue(app.buttons["settings.restartToUpdate"].waitForExistence(timeout: 5))
    }

    func testMenuShowsUpdateStatusAndActions() throws {
        let app = launch(updater: "staged")
        XCTAssertTrue(app.windows.firstMatch.waitForExistence(timeout: 10))

        let menu = try openStatusMenu(app)

        XCTAssertTrue(menu.menuItems["Open Ambit"].exists)
        XCTAssertTrue(menu.menuItems["Ambit 99.0 is ready to install"].exists)
        XCTAssertTrue(menu.menuItems["Restart to Update"].exists)
        XCTAssertTrue(menu.menuItems["Quit Ambit"].exists)
        app.typeKey(.escape, modifierFlags: [])
    }
}
