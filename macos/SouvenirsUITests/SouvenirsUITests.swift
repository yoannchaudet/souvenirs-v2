import XCTest

final class SouvenirsUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testCreatesCatalogWithFakeEngine() throws {
        let app = launch(scenario: "catalog-flow")

        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["engine-status"].waitForExistence(timeout: 5))
        app.buttons["create-catalog"].click()
        XCTAssertTrue(app.textFields["catalog-name"].waitForExistence(timeout: 2))

        app.buttons["choose-catalog-location"].click()
        XCTAssertTrue(
            app.staticTexts["catalog-location-valid"].waitForExistence(timeout: 3)
        )
        let create = app.buttons["confirm-create-catalog"]
        expectation(
            for: NSPredicate(format: "isEnabled == true"),
            evaluatedWith: create
        )
        waitForExpectations(timeout: 3)
        app.buttons["confirm-create-catalog"].click()

        XCTAssertTrue(app.staticTexts["catalog-ready"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["catalog-health"].exists)
    }

    @MainActor
    func testStorageValidationFailureIsActionable() throws {
        let app = launch(scenario: "catalog-error")

        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["create-catalog"].click()
        app.buttons["choose-catalog-location"].click()

        XCTAssertTrue(
            app.staticTexts["catalog-location-error"].waitForExistence(timeout: 3)
        )
        XCTAssertFalse(app.buttons["confirm-create-catalog"].isEnabled)
    }

    @MainActor
    func testReviewsImportAndResolvesRequiredDecision() throws {
        let app = launch(scenario: "import-flow")

        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()
        XCTAssertTrue(app.staticTexts["catalog-ready"].waitForExistence(timeout: 5))
        app.buttons["choose-import-folder"].click()

        XCTAssertTrue(app.descendants(matching: .any)["import-review-sheet"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["import-review-title"].exists)
        XCTAssertTrue(app.descendants(matching: .any)["import-date-decision"].exists)
        XCTAssertFalse(app.buttons["start-import"].isEnabled)

        app.buttons["date-as-shown"].click()
        let start = app.buttons["start-import"]
        expectation(
            for: NSPredicate(format: "isEnabled == true"),
            evaluatedWith: start
        )
        waitForExpectations(timeout: 3)
        app.disclosureTriangles["import-details"].click()
        XCTAssertTrue(app.descendants(matching: .any)["import-operation-summary"].exists)
        start.click()
        XCTAssertEqual(start.label, "Plan Ready")
    }

    @MainActor
    func testImportSourceFailureBlocksPlanning() throws {
        let app = launch(scenario: "import-error")

        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()
        XCTAssertTrue(app.staticTexts["catalog-ready"].waitForExistence(timeout: 5))
        app.buttons["choose-import-folder"].click()

        XCTAssertTrue(
            app.staticTexts["import-planning-error"].waitForExistence(timeout: 5)
        )
        XCTAssertFalse(app.buttons["start-import"].isEnabled)
    }

    @MainActor
    private func launch(scenario: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "-ApplePersistenceIgnoreState", "YES",
            "-NSQuitAlwaysKeepsWindows", "NO"
        ]
        app.launchEnvironment["SOUVENIRS_ENGINE_EXECUTABLE"] =
            repositoryRoot.appendingPathComponent(
                "target/debug/souvenirs-fake-engine"
            ).path
        app.launchEnvironment["SOUVENIRS_FAKE_ENGINE_SCENARIO"] = scenario
        app.launchEnvironment["SOUVENIRS_UI_TEST_DIRECTORY"] =
            "/Volumes/photos/Family Catalog"
        app.launch()
        return app
    }

    private var repositoryRoot: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
    }
}
