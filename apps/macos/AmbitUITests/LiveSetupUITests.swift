import XCTest

@MainActor
final class LiveSetupUITests: XCTestCase {
    private let sandbox = FileManager.default.temporaryDirectory
        .appending(path: "ambit-live-ui-\(UUID().uuidString)", directoryHint: .isDirectory)

    private var home: URL { sandbox.appending(path: "home") }
    private var skill: URL { home.appending(path: ".agents/skills/review") }

    override func setUpWithError() throws {
        continueAfterFailure = false
        let source = home.appending(path: "catalog/skills/review")
        try FileManager.default.createDirectory(at: source, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: sandbox.appending(path: "support"), withIntermediateDirectories: true)
        try "---\nname: review\ndescription: Reviews code.\n---\n\n# Review\n".write(
            to: source.appending(path: "SKILL.md"), atomically: true, encoding: .utf8)
        try """
            version: 1
            harnesses: [claude]
            catalogs:
              - name: team
                source: path:./catalog
            requires: []

            """.write(to: home.appending(path: "ambit.yml"), atomically: true, encoding: .utf8)
        try "keep".write(to: home.appending(path: "unrelated.txt"), atomically: true, encoding: .utf8)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: sandbox)
    }

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["AMBIT_UI_TESTING"] = "1"
        app.launchEnvironment["AMBIT_TEST_ENGINE"] = "live"
        app.launchEnvironment["AMBIT_TEST_HOME"] = home.path
        app.launchEnvironment["AMBIT_TEST_APP_SUPPORT"] = sandbox.appending(path: "support").path
        app.launch()
        return app
    }

    private func element(_ identifier: String, in app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    private func click(_ identifier: String, in app: XCUIApplication) {
        let target = element(identifier, in: app)
        XCTAssertTrue(target.waitForExistence(timeout: 20), "\(identifier) is missing")
        target.click()
    }

    func testInstallsAndRemovesALocalSkillThroughTheLiveEngine() throws {
        let app = launch()
        let row = "capability.row.skill.team/review"
        click(row, in: app)
        click("capability.select", in: app)
        click("setup.apply", in: app)
        click("review.apply", in: app)
        click("review.done", in: app)

        XCTAssertTrue(FileManager.default.fileExists(atPath: skill.appending(path: "SKILL.md").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: home.appending(path: ".claude/skills/review/SKILL.md").path))

        click(row, in: app)
        click("capability.remove", in: app)
        click("setup.apply", in: app)
        click("review.apply", in: app)
        click("review.done", in: app)

        XCTAssertFalse(FileManager.default.fileExists(atPath: skill.path))
        XCTAssertEqual(try String(contentsOf: home.appending(path: "unrelated.txt"), encoding: .utf8), "keep")
    }
}
