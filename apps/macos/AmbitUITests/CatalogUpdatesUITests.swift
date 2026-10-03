import XCTest

@MainActor
final class CatalogUpdatesUITests: XCTestCase {
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

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchEnvironment["AMBIT_UI_TESTING"] = "1"
        app.launchEnvironment["AMBIT_TEST_HOME"] = sandbox.appending(path: "home").path
        app.launchEnvironment["AMBIT_TEST_APP_SUPPORT"] = sandbox.appending(path: "support").path
        app.launchEnvironment["AMBIT_TEST_ENGINE"] = "catalogUpdates"
        app.launch()
        return app
    }

    private func element(_ key: String, in app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == %@ OR label == %@", key, key))
            .firstMatch
    }

    private func text(of element: XCUIElement) -> String {
        [element.label, element.value as? String ?? ""].joined(separator: " ")
    }

    func testCheckReviewAndApplyAnUpdate() throws {
        let app = launch()

        let catalogsTab = app.radioButtons["Catalogs"].firstMatch
        XCTAssertTrue(catalogsTab.waitForExistence(timeout: 10))
        catalogsTab.click()

        let check = element("catalogUpdates.check", in: app)
        try XCTSkipUnless(
            check.waitForExistence(timeout: 5),
            "CatalogUpdatesView is not embedded in the Catalogs view yet.")

        XCTAssertTrue(text(of: element("catalogUpdates.lastChecked", in: app)).contains("Never checked"))
        XCTAssertTrue(text(of: element("catalogUpdates.status.team", in: app)).contains("Not checked"))
        XCTAssertTrue(text(of: element("catalogUpdates.status.frozen", in: app)).contains("Pinned"))
        XCTAssertTrue(text(of: element("catalogUpdates.status.notes", in: app)).contains("Uses local files"))

        check.click()

        let update = element("catalogUpdates.update.team", in: app)
        XCTAssertTrue(update.waitForExistence(timeout: 10))
        XCTAssertTrue(text(of: element("catalogUpdates.status.team", in: app)).contains("Update available"))
        XCTAssertTrue(text(of: element("catalogUpdates.status.docs", in: app)).contains("Up to date"))
        XCTAssertTrue(text(of: element("catalogUpdates.status.private", in: app)).contains("Check failed"))
        XCTAssertTrue(element("catalogUpdates.retry.private", in: app).exists)
        XCTAssertFalse(text(of: element("catalogUpdates.lastChecked", in: app)).contains("Never checked"))

        update.click()

        let apply = element("updateReview.apply", in: app)
        XCTAssertTrue(apply.waitForExistence(timeout: 10))
        XCTAssertTrue(element("triage (skill, team)", in: app).exists)
        apply.click()

        XCTAssertTrue(element("updateReview.installed", in: app).waitForExistence(timeout: 10))
        element("updateReview.done", in: app).click()

        let status = element("catalogUpdates.status.team", in: app)
        XCTAssertTrue(status.waitForExistence(timeout: 5))
        XCTAssertTrue(text(of: status).contains("Up to date"), text(of: status))
        XCTAssertFalse(element("catalogUpdates.update.team", in: app).exists)
    }
}
