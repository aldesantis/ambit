import XCTest

@MainActor
final class SetupFlowUITests: XCTestCase {
    /// XCTest creates one instance per test, so every test gets its own sandbox.
    private let sandbox = FileManager.default.temporaryDirectory
        .appending(path: "ambit-ui-\(UUID().uuidString)", directoryHint: .isDirectory)

    private var home: URL { sandbox.appending(path: "home") }

    override func setUpWithError() throws {
        continueAfterFailure = false
        for name in ["home", "support", "projects/demo"] {
            try FileManager.default.createDirectory(
                at: sandbox.appending(path: name), withIntermediateDirectories: true)
        }
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: sandbox)
    }

    private func launch(pickFolder: URL? = nil) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["AMBIT_UI_TESTING"] = "1"
        app.launchEnvironment["AMBIT_TEST_HOME"] = home.path
        app.launchEnvironment["AMBIT_TEST_APP_SUPPORT"] = sandbox.appending(path: "support").path
        if let pickFolder {
            app.launchEnvironment["AMBIT_TEST_PICK_FOLDER"] = pickFolder.path
        }
        app.launch()
        return app
    }

    /// Finds a test identifier or visible text without reading every element's label.
    private func element(_ key: String, in app: XCUIApplication) -> XCUIElement {
        if key.contains(".") {
            return app.descendants(matching: .any).matching(identifier: key).firstMatch
        }
        return app.staticTexts[key]
    }

    private func click(_ key: String, in app: XCUIApplication, file: StaticString = #filePath, line: UInt = #line) {
        let target = element(key, in: app)
        XCTAssertTrue(target.waitForExistence(timeout: 10), "\(key) does not exist", file: file, line: line)
        target.click()
    }

    private func badgeLabel(in app: XCUIApplication) -> String {
        let badge = element("setup.badge", in: app)
        XCTAssertTrue(badge.waitForExistence(timeout: 10))
        return (badge.value as? String ?? "") + " " + badge.label
    }

    private func homeContents() throws -> [String] {
        try FileManager.default.contentsOfDirectory(atPath: home.path)
    }

    func testCreatesAnEmptyPersonalSetup() throws {
        let app = launch()

        XCTAssertTrue(badgeLabel(in: app).contains("Not configured"), badgeLabel(in: app))
        let next = element("newSetup.continue", in: app)
        XCTAssertTrue(next.waitForExistence(timeout: 10))
        XCTAssertFalse(next.isEnabled, "Continue needs at least one agent tool")

        click("newSetup.tool.claude", in: app)
        click("newSetup.tool.codex", in: app)
        XCTAssertTrue(next.isEnabled)
        next.click()

        click("newSetup.skip", in: app)
        let apply = element("review.apply", in: app)
        XCTAssertTrue(apply.waitForExistence(timeout: 10))
        XCTAssertTrue(element("review.summary", in: app).exists)
        XCTAssertEqual(try homeContents(), [], "Reviewing must not write")
        apply.click()

        XCTAssertTrue(element("review.result", in: app).waitForExistence(timeout: 10))
        click("review.done", in: app)

        let installed = NSPredicate(format: "label CONTAINS %@ OR value CONTAINS %@", "Installed", "Installed")
        expectation(for: installed, evaluatedWith: element("setup.badge", in: app))
        waitForExpectations(timeout: 10)
        XCTAssertFalse(element("newSetup.continue", in: app).exists)
        XCTAssertFalse(element("setup.apply", in: app).exists)
    }

    func testCancelingTheNewSetupLeavesHomeUntouched() throws {
        let app = launch()

        click("newSetup.tool.cursor", in: app)
        click("newSetup.continue", in: app)
        XCTAssertTrue(element("setup.apply", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(badgeLabel(in: app).contains("Pending changes"), badgeLabel(in: app))

        click("newSetup.cancel", in: app)

        XCTAssertTrue(element("newSetup.tool.cursor", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(badgeLabel(in: app).contains("Not configured"))
        XCTAssertFalse(element("setup.apply", in: app).exists)
        XCTAssertEqual(try homeContents(), [])
    }

    func testAddsAProjectAndOpensItsSetup() throws {
        let app = launch(pickFolder: sandbox.appending(path: "projects/demo"))

        click("sidebar.addProject", in: app)

        let name = element("setup.name", in: app)
        let shown = NSPredicate(format: "label == %@ OR value == %@", "demo", "demo")
        expectation(for: shown, evaluatedWith: name)
        waitForExpectations(timeout: 10)
        XCTAssertTrue(element("newSetup.tool.claude", in: app).waitForExistence(timeout: 10))

        // Adding the same folder again selects the existing entry instead of duplicating it.
        click("sidebar.personal", in: app)
        click("sidebar.addProject", in: app)
        expectation(for: shown, evaluatedWith: name)
        waitForExpectations(timeout: 10)
    }

    func testAddingTheHomeFolderOpensPersonalSetup() throws {
        let app = launch(pickFolder: home)

        click("sidebar.addProject", in: app)

        let name = element("setup.name", in: app)
        let personal = NSPredicate(format: "label == %@ OR value == %@", "Personal setup", "Personal setup")
        expectation(for: personal, evaluatedWith: name)
        waitForExpectations(timeout: 10)
        XCTAssertFalse(element("home", in: app).exists)
    }

    func testSwitchingAwayFromPendingChangesAsks() throws {
        let app = launch(pickFolder: sandbox.appending(path: "projects/demo"))
        click("sidebar.addProject", in: app)
        XCTAssertTrue(element("newSetup.tool.claude", in: app).waitForExistence(timeout: 10))

        click("newSetup.tool.claude", in: app)
        click("newSetup.continue", in: app)
        XCTAssertTrue(element("setup.apply", in: app).waitForExistence(timeout: 5))

        click("sidebar.personal", in: app)
        click("pending.cancel", in: app)
        XCTAssertTrue(element("setup.apply", in: app).waitForExistence(timeout: 5))
        XCTAssertTrue(badgeLabel(in: app).contains("Pending changes"))

        XCTAssertEqual(try homeContents(), [])
    }
}
