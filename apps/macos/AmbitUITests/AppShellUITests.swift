import XCTest

@MainActor
final class AppShellUITests: XCTestCase {
    /// XCTest creates one instance per test, so every test gets its own sandbox.
    private let sandbox = FileManager.default.temporaryDirectory
        .appending(path: "ambit-ui-\(UUID().uuidString)", directoryHint: .isDirectory)

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
        app.launchEnvironment["AMBIT_TEST_HOME"] = sandbox.appending(path: "home").path
        app.launchEnvironment["AMBIT_TEST_APP_SUPPORT"] = sandbox.appending(path: "support").path
        if let pickFolder {
            app.launchEnvironment["AMBIT_TEST_PICK_FOLDER"] = pickFolder.path
        }
        app.launch()
        return app
    }

    /// The first element whose identifier or label is `key`, whatever its type.
    private func element(_ key: String, in app: XCUIApplication) -> XCUIElement {
        app.windows.firstMatch.descendants(matching: .any)
            .matching(NSPredicate(format: "identifier == %@ OR label == %@", key, key))
            .firstMatch
    }

    func testShowsPersonalSetupFromTheTestHome() throws {
        let app = launch()

        XCTAssertTrue(element("Personal setup", in: app).waitForExistence(timeout: 10))
        XCTAssertTrue(element("sidebar.addProject", in: app).exists)

        let root = element("setup.root", in: app)
        XCTAssertTrue(root.waitForExistence(timeout: 5))
        // SwiftUI exposes a text's accessibility label as the static text's value.
        let shown = root.value as? String ?? root.label
        XCTAssertTrue(shown.hasSuffix(sandbox.appending(path: "home").path), shown)
        XCTAssertTrue(element("setup.badge", in: app).waitForExistence(timeout: 5))
    }

    func testAddedProjectIsRememberedAcrossLaunches() throws {
        var app = launch(pickFolder: sandbox.appending(path: "projects/demo"))

        element("sidebar.addProject", in: app).click()
        XCTAssertTrue(element("demo", in: app).waitForExistence(timeout: 5))
        app.terminate()

        app = launch()
        XCTAssertTrue(element("demo", in: app).waitForExistence(timeout: 10))
    }
}
