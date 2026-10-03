import XCTest

@MainActor
final class AgentToolsUITests: XCTestCase {
    private let sandbox = FileManager.default.temporaryDirectory
        .appending(path: "ambit-ui-\(UUID().uuidString)", directoryHint: .isDirectory)

    override func setUpWithError() throws {
        continueAfterFailure = false
        for name in ["home", "support"] {
            try FileManager.default.createDirectory(
                at: sandbox.appending(path: name), withIntermediateDirectories: true)
        }
        let config = "version: 1\nharnesses:\n  - claude\ncatalogs: []\nrequires: []\n"
        try config.write(to: sandbox.appending(path: "home/ambit.yml"), atomically: true, encoding: .utf8)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: sandbox)
    }

    /// The first element whose identifier, label or value is `key`. SwiftUI exposes a text as a
    /// static text whose value, not label, holds the string.
    private func element(_ key: String, in app: XCUIApplication) -> XCUIElement {
        app.windows.firstMatch.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == %@ OR label == %@ OR value == %@", key, key, key))
            .firstMatch
    }

    func testTurningOnASecondToolStagesAPendingChange() throws {
        let app = XCUIApplication()
        app.launchEnvironment["AMBIT_UI_TESTING"] = "1"
        app.launchEnvironment["AMBIT_TEST_HOME"] = sandbox.appending(path: "home").path
        app.launchEnvironment["AMBIT_TEST_APP_SUPPORT"] = sandbox.appending(path: "support").path
        app.launch()

        let badge = element("setup.badge", in: app)
        XCTAssertTrue(badge.waitForExistence(timeout: 10))
        element("Agent Tools", in: app).click()

        // The only selected tool cannot be turned off.
        let claude = element("agentTools.toggle.claude", in: app)
        XCTAssertTrue(claude.waitForExistence(timeout: 5))
        claude.click()
        XCTAssertTrue(element("agentTools.refusal", in: app).waitForExistence(timeout: 5))

        element("agentTools.toggle.codex", in: app).click()

        XCTAssertTrue(element("Will be added", in: app).waitForExistence(timeout: 5))
        let pending = NSPredicate(format: "value CONTAINS[c] 'Pending changes' OR label CONTAINS[c] 'Pending changes'")
        expectation(for: pending, evaluatedWith: badge)
        waitForExpectations(timeout: 5)
        XCTAssertFalse(element("agentTools.refusal", in: app).exists)
    }
}
