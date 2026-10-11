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
    func testStartupFailureDisablesCatalogActionsAndSupportsRetry() throws {
        let app = launch(scenario: "startup-failure-once")

        XCTAssertTrue(
            app.descendants(matching: .any)["catalog-actions-unavailable"]
                .waitForExistence(timeout: 5)
        )
        XCTAssertFalse(app.buttons["create-catalog"].isEnabled)
        XCTAssertFalse(app.buttons["open-catalog"].isEnabled)
        XCTAssertTrue(app.buttons["retry-engine"].isEnabled)

        app.typeKey(.return, modifierFlags: [])
        XCTAssertFalse(app.textFields["catalog-name"].exists)

        app.buttons["retry-engine"].click()
        XCTAssertTrue(app.buttons["retry-engine"].waitForNonExistence(timeout: 2))
        expectation(
            for: NSPredicate(format: "isEnabled == true"),
            evaluatedWith: app.buttons["create-catalog"]
        )
        waitForExpectations(timeout: 5)
        XCTAssertFalse(
            app.descendants(matching: .any)["catalog-actions-unavailable"].exists
        )
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

        let disclosure = app.buttons["created-items-disclosure"]
        XCTAssertTrue(disclosure.exists)
        disclosure.click()
        XCTAssertEqual(disclosure.value as? String, "Expanded")
        XCTAssertTrue(
            app.descendants(matching: .any)["created-items-description"]
                .waitForExistence(timeout: 2)
        )
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = "catalog-create-expanded-invalid"
        attachment.lifetime = .keepAlways
        add(attachment)
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
        XCTAssertFalse(app.descendants(matching: .any)["start-import"].isEnabled)

        app.descendants(matching: .any)["date-as-shown"].click()
        let start = app.descendants(matching: .any)["start-import"]
        expectation(
            for: NSPredicate(format: "isEnabled == true"),
            evaluatedWith: start
        )
        waitForExpectations(timeout: 3)
        app.descendants(matching: .any)["import-details"].click()
        XCTAssertTrue(app.descendants(matching: .any)["import-operation-summary"].exists)
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
        XCTAssertFalse(app.descendants(matching: .any)["start-import"].isEnabled)
    }

    @MainActor
    func testExecutesImportAndShowsCompletedOutcome() throws {
        let app = launch(scenario: "import-execution")
        openImportReview(app)
        app.descendants(matching: .any)["date-as-shown"].click()
        let start = app.descendants(matching: .any)["start-import"]
        expectation(
            for: NSPredicate(format: "isEnabled == true"),
            evaluatedWith: start
        )
        waitForExpectations(timeout: 3)
        app.typeKey(.return, modifierFlags: [])

        XCTAssertTrue(app.staticTexts["import-completed"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["dismiss-import-completed"].exists)
    }

    @MainActor
    func testExecutionSourceDisconnectShowsReconnectState() throws {
        let app = launch(scenario: "import-source-unavailable")
        openImportReview(app)
        app.descendants(matching: .any)["date-as-shown"].click()
        let start = app.descendants(matching: .any)["start-import"]
        expectation(
            for: NSPredicate(format: "isEnabled == true"),
            evaluatedWith: start
        )
        waitForExpectations(timeout: 3)
        app.typeKey(.return, modifierFlags: [])

        XCTAssertTrue(app.staticTexts["recovery-title"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["recovery-source-unavailable"].exists)
        XCTAssertTrue(app.buttons["retry-recovery"].isEnabled)
        XCTAssertFalse(app.buttons["finish-recovery"].isEnabled)
    }

    @MainActor
    func testRoutesUnfinishedImportAndMissingSource() throws {
        let app = launch(scenario: "import-recovery-missing")
        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()

        XCTAssertTrue(app.staticTexts["recovery-title"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["recovery-source-unavailable"].exists)
        XCTAssertTrue(app.buttons["retry-recovery"].isEnabled)
        XCTAssertFalse(app.buttons["finish-recovery"].isEnabled)
        XCTAssertTrue(app.buttons["cancel-recovery"].isEnabled)
        app.buttons["defer-recovery"].click()
        XCTAssertTrue(
            app.staticTexts["import-recovery-deferred"].waitForExistence(timeout: 5)
        )
        XCTAssertTrue(app.buttons["retry-deferred-recovery"].exists)
        XCTAssertTrue(app.buttons["cancel-deferred-recovery"].exists)
        XCTAssertFalse(app.buttons["choose-import-folder"].isEnabled)
    }

    @MainActor
    func testFinishesDetectedRecovery() throws {
        let app = launch(scenario: "import-recovery")
        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()
        XCTAssertTrue(app.staticTexts["recovery-title"].waitForExistence(timeout: 5))
        app.buttons["finish-recovery"].click()

        XCTAssertTrue(app.staticTexts["import-completed"].waitForExistence(timeout: 5))
    }

    @MainActor
    private func openImportReview(_ app: XCUIApplication) {
        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()
        XCTAssertTrue(app.staticTexts["catalog-ready"].waitForExistence(timeout: 5))
        app.buttons["choose-import-folder"].click()
        XCTAssertTrue(
            app.descendants(matching: .any)["import-review-sheet"]
                .waitForExistence(timeout: 5)
        )
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
        if scenario == "startup-failure-once" {
            let marker = URL(fileURLWithPath: NSTemporaryDirectory())
                .appendingPathComponent("SouvenirsUITests-\(UUID().uuidString)")
            app.launchEnvironment["SOUVENIRS_FAKE_ENGINE_RETRY_MARKER"] = marker.path
            addTeardownBlock {
                try? FileManager.default.removeItem(at: marker)
            }
        }
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
