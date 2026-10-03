import XCTest

@MainActor
final class CapabilitiesUITests: XCTestCase {
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

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["AMBIT_UI_TESTING"] = "1"
        app.launchEnvironment["AMBIT_TEST_HOME"] = sandbox.appending(path: "home").path
        app.launchEnvironment["AMBIT_TEST_APP_SUPPORT"] = sandbox.appending(path: "support").path
        app.launchEnvironment["AMBIT_TEST_ENGINE"] = "capabilities"
        app.launch()
        return app
    }

    private func element(_ key: String, in app: XCUIApplication) -> XCUIElement {
        app.windows.firstMatch.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == %@ OR label == %@", key, key))
            .firstMatch
    }

    func testSelectingASkillStagesIt() throws {
        let app = launch()

        let row = element("capability.row.skill.team/review", in: app)
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        row.click()

        XCTAssertTrue(element("capability.skillDocument", in: app).waitForExistence(timeout: 5))
        let select = element("capability.select", in: app)
        XCTAssertTrue(select.waitForExistence(timeout: 5))
        select.click()

        let badge = element("setup.badge", in: app)
        // SwiftUI exposes a text's accessibility label as the static text's value.
        let staged = NSPredicate(
            format: "label CONTAINS[c] %@ OR value CONTAINS[c] %@", "Pending changes", "Pending changes")
        expectation(for: staged, evaluatedWith: badge)
        waitForExpectations(timeout: 5)

        // The skill is now selected directly, so the detail offers removal instead.
        XCTAssertTrue(element("capability.remove", in: app).waitForExistence(timeout: 5))
        XCTAssertFalse(element("capability.select", in: app).exists)
    }
}
