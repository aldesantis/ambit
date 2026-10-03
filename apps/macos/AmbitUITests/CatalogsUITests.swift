import XCTest

@MainActor
final class CatalogsUITests: XCTestCase {
    private nonisolated let sandbox = FileManager.default.temporaryDirectory
        .appending(path: "ambit-ui-\(UUID().uuidString)", directoryHint: .isDirectory)

    private nonisolated var home: URL { sandbox.appending(path: "home") }
    private nonisolated var catalog: URL { home.appending(path: "team-catalog") }

    private static let config = """
        version: 1
        harnesses:
          - claude
        catalogs: []
        requires: []

        """

    override func setUpWithError() throws {
        continueAfterFailure = false
        let skill = catalog.appending(path: "skills/review")
        try FileManager.default.createDirectory(at: skill, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: sandbox.appending(path: "support"), withIntermediateDirectories: true)
        try "---\nname: review\ndescription: Reviews code.\n---\n\n# Review\n".write(
            to: skill.appending(path: "SKILL.md"), atomically: true, encoding: .utf8)
        try Self.config.write(to: home.appending(path: "ambit.yml"), atomically: true, encoding: .utf8)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: sandbox)
    }

    private func element(_ key: String, in app: XCUIApplication) -> XCUIElement {
        app.windows.firstMatch.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == %@ OR label == %@", key, key))
            .firstMatch
    }

    func testAddsALocalCatalogToThePersonalSetupAsAPendingChange() throws {
        let app = XCUIApplication()
        app.launchEnvironment["AMBIT_UI_TESTING"] = "1"
        app.launchEnvironment["AMBIT_TEST_HOME"] = home.path
        app.launchEnvironment["AMBIT_TEST_APP_SUPPORT"] = sandbox.appending(path: "support").path
        app.launchEnvironment["AMBIT_TEST_PICK_FOLDER"] = catalog.path
        app.launch()

        XCTAssertTrue(element("Personal setup", in: app).waitForExistence(timeout: 10))
        let tab = app.windows.firstMatch.tabs["Catalogs"]
        XCTAssertTrue(tab.waitForExistence(timeout: 5))
        tab.click()

        let add = element("catalogs.add", in: app)
        XCTAssertTrue(add.waitForExistence(timeout: 5))
        add.click()

        let choose = element("catalogEditor.chooseFolder", in: app)
        XCTAssertTrue(choose.waitForExistence(timeout: 5))
        choose.click()

        let source = element("catalogEditor.source", in: app)
        let picked = catalog.resolvingSymlinksInPath().path
        XCTAssertTrue(
            NSPredicate(format: "value == %@", picked).evaluate(with: source)
                || waitForValue(picked, of: source), String(describing: source.value))
        let name = element("catalogEditor.name", in: app)
        XCTAssertEqual(name.value as? String, "team-catalog")
        XCTAssertFalse(element("catalogEditor.revision", in: app).exists, "local folders have no revision")
        XCTAssertFalse(element("catalogEditor.save", in: app).isEnabled, "the source must be verified first")

        element("catalogEditor.verify", in: app).click()
        XCTAssertTrue(element("catalogEditor.verified", in: app).waitForExistence(timeout: 30))

        let save = element("catalogEditor.save", in: app)
        XCTAssertTrue(save.isEnabled)
        save.click()

        XCTAssertTrue(element("catalog.row.team-catalog", in: app).waitForExistence(timeout: 5))
        let badge = element("setup.badge", in: app)
        XCTAssertTrue(badge.waitForExistence(timeout: 5))
        // SwiftUI exposes a text's accessibility label as the static text's value.
        let status = (badge.value as? String ?? "") + badge.label
        XCTAssertTrue(status.contains("Pending changes"), status)

        // Staging never writes: the saved config is untouched.
        let saved = try String(contentsOf: home.appending(path: "ambit.yml"), encoding: .utf8)
        XCTAssertEqual(saved, Self.config)
    }

    private func waitForValue(_ value: String, of element: XCUIElement) -> Bool {
        let expectation = XCTNSPredicateExpectation(
            predicate: NSPredicate(format: "value == %@", value), object: element)
        return XCTWaiter.wait(for: [expectation], timeout: 5) == .completed
    }
}
