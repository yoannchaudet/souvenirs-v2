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
        let launcher = app.buttons["create-catalog"]
        launcher.click()
        let sheet = app.sheets.firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 2))
        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].exists)
        let name = app.textFields["catalog-name"]
        XCTAssertTrue(name.waitForExistence(timeout: 2))
        expectation(
            for: NSPredicate(format: "hasKeyboardFocus == true"),
            evaluatedWith: name
        )
        waitForExpectations(timeout: 3)

        app.typeKey(.escape, modifierFlags: [])
        XCTAssertTrue(sheet.waitForNonExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].exists)
        expectation(
            for: NSPredicate(format: "hasKeyboardFocus == true"),
            evaluatedWith: launcher
        )
        waitForExpectations(timeout: 3)

        launcher.click()
        XCTAssertTrue(app.sheets.firstMatch.waitForExistence(timeout: 2))

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
        XCTAssertTrue(app.sheets.firstMatch.waitForExistence(timeout: 2))
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
    func testReviewsImportWithOptionalDateDecision() throws {
        let app = launch(scenario: "import-flow")

        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()
        XCTAssertTrue(app.staticTexts["catalog-ready"].waitForExistence(timeout: 5))
        app.buttons["choose-import-folder"].click()

        let sheet = activeSheet(app)
        XCTAssertTrue(
            sheet.descendants(matching: .any)["import-review-sheet"]
                .waitForExistence(timeout: 5)
        )
        XCTAssertTrue(sheet.staticTexts["import-review-title"].exists)
        XCTAssertTrue(
            sheet.descendants(matching: .any)["import-date-decision"].exists
        )
        XCTAssertTrue(sheet.buttons["start-import"].isEnabled)
        XCTAssertTrue(sheet.staticTexts["Example · As shown: 12:14"].exists)
        XCTAssertTrue(sheet.staticTexts["Example · Convert from UTC: 14:14"].exists)

        sheet.buttons["date-as-shown"].click()
        XCTAssertTrue(
            sheet.descendants(matching: .any)["import-date-selection"]
                .waitForExistence(timeout: 3)
        )
        XCTAssertTrue(sheet.buttons["change-date-selection"].isEnabled)
        XCTAssertEqual(
            sheet.descendants(matching: .any)["import-date-selection"]
                .value as? String,
            "As shown"
        )
        sheet.buttons["change-date-selection"].click()
        XCTAssertTrue(sheet.buttons["date-convert-utc"].waitForExistence(timeout: 3))
        sheet.buttons["import-details"].click()
        XCTAssertTrue(
            sheet.descendants(matching: .any)["import-operation-summary"].exists
        )
        for identifier in [
            "import-detail-destination",
            "import-detail-file-0",
            "import-detail-space",
            "import-detail-write-probe",
            "import-detail-portable-names",
            "import-detail-no-replacement",
            "import-detail-copy-sequence",
            "import-detail-recovery"
        ] {
            XCTAssertTrue(
                sheet.descendants(matching: .any)[identifier]
                    .waitForExistence(timeout: 3),
                "\(identifier) should appear in the expanded import details"
            )
        }
    }

    @MainActor
    func testImportPlanningSheetContainsFocusAndEscapeRestoresLauncher() throws {
        let app = launch(scenario: "import-flow")

        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()
        XCTAssertTrue(app.staticTexts["catalog-ready"].waitForExistence(timeout: 5))
        let launcher = app.buttons["choose-import-folder"]
        launcher.click()

        let sheet = app.sheets.firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 5))
        XCTAssertTrue(
            sheet.descendants(matching: .any)["import-review-sheet"]
                .waitForExistence(timeout: 5)
        )
        XCTAssertEqual(app.sheets.count, 1)
        XCTAssertFalse(launcher.isHittable)
        let collectionName = app.textFields["import-collection-name"]
        XCTAssertTrue(collectionName.waitForExistence(timeout: 5))
        expectation(
            for: NSPredicate(format: "hasKeyboardFocus == true"),
            evaluatedWith: collectionName
        )
        waitForExpectations(timeout: 3)

        app.typeKey(.escape, modifierFlags: [])

        XCTAssertTrue(sheet.waitForNonExistence(timeout: 3))
        expectation(
            for: NSPredicate(format: "hasKeyboardFocus == true"),
            evaluatedWith: launcher
        )
        waitForExpectations(timeout: 3)
    }

    @MainActor
    func testImportSourceFailureRetriesInPlaceAndRestoresReviewFocus() throws {
        let app = launch(scenario: "import-error")

        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()
        XCTAssertTrue(app.staticTexts["catalog-ready"].waitForExistence(timeout: 5))
        app.buttons["choose-import-folder"].click()

        XCTAssertTrue(
            app.staticTexts["import-planning-error"].waitForExistence(timeout: 5)
        )
        let sheet = activeSheet(app)
        XCTAssertTrue(sheet.staticTexts["import-review-title"].exists)
        XCTAssertFalse(sheet.buttons["start-import"].isEnabled)
        XCTAssertTrue(sheet.staticTexts["import-start-blocked"].exists)
        let retry = sheet.buttons["retry-import-scan"]
        XCTAssertTrue(retry.isEnabled)
        expectation(
            for: NSPredicate(format: "hasKeyboardFocus == true"),
            evaluatedWith: retry
        )
        waitForExpectations(timeout: 3)

        retry.click()

        XCTAssertEqual(app.sheets.count, 1)
        XCTAssertTrue(
            sheet.staticTexts["import-planning-error"].waitForNonExistence(timeout: 3)
        )
        let collectionName = sheet.textFields["import-collection-name"]
        XCTAssertTrue(collectionName.waitForExistence(timeout: 5))
        expectation(
            for: NSPredicate(format: "hasKeyboardFocus == true"),
            evaluatedWith: collectionName
        )
        waitForExpectations(timeout: 3)
        expectation(
            for: NSPredicate(format: "isEnabled == true"),
            evaluatedWith: sheet.buttons["start-import"]
        )
        waitForExpectations(timeout: 3)
    }

    @MainActor
    func testExecutesImportAndShowsCompletedOutcome() throws {
        let app = launch(scenario: "import-execution")
        let sheet = openImportReview(app)
        let start = sheet.buttons["start-import"]
        expectation(
            for: NSPredicate(format: "isEnabled == true"),
            evaluatedWith: start
        )
        waitForExpectations(timeout: 3)
        start.click()

        XCTAssertTrue(app.staticTexts["import-completed"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["dismiss-import-completed"].exists)
    }

    @MainActor
    func testExecutionSourceDisconnectShowsReconnectState() throws {
        let app = launch(scenario: "import-source-unavailable")
        let sheet = openImportReview(app)
        sheet.buttons["date-as-shown"].click()
        let start = sheet.buttons["start-import"]
        expectation(
            for: NSPredicate(format: "isEnabled == true"),
            evaluatedWith: start
        )
        waitForExpectations(timeout: 3)
        start.click()

        let recoverySheet = activeSheet(app)
        XCTAssertTrue(
            recoverySheet.staticTexts["recovery-title"].waitForExistence(timeout: 5)
        )
        XCTAssertTrue(recoverySheet.staticTexts["recovery-source-unavailable"].exists)
        XCTAssertTrue(recoverySheet.buttons["retry-recovery"].isEnabled)
        XCTAssertFalse(recoverySheet.buttons["finish-recovery"].isEnabled)
    }

    @MainActor
    func testRoutesUnfinishedImportAndMissingSource() throws {
        let app = launch(scenario: "import-recovery-missing")
        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()

        let sheet = activeSheet(app)
        XCTAssertTrue(sheet.staticTexts["recovery-title"].waitForExistence(timeout: 5))
        XCTAssertEqual(app.sheets.count, 1)
        XCTAssertTrue(sheet.staticTexts["recovery-source-unavailable"].exists)
        XCTAssertTrue(sheet.buttons["retry-recovery"].isEnabled)
        XCTAssertFalse(sheet.buttons["finish-recovery"].isEnabled)
        XCTAssertTrue(sheet.buttons["cancel-recovery"].isEnabled)
        sheet.buttons["defer-recovery"].click()
        XCTAssertTrue(
            app.staticTexts["import-recovery-deferred"].waitForExistence(timeout: 5)
        )
        XCTAssertTrue(app.buttons["retry-deferred-recovery"].exists)
        XCTAssertTrue(app.buttons["cancel-deferred-recovery"].exists)
        XCTAssertFalse(app.buttons["choose-import-folder"].isEnabled)
    }

    @MainActor
    func testRecoveryEscapeDefersAndRestoresRetryLauncher() throws {
        let app = launch(scenario: "import-recovery-missing")
        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()

        let sheet = app.sheets.firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 5))
        XCTAssertTrue(
            sheet.staticTexts["recovery-title"]
                .waitForExistence(timeout: 5)
        )
        XCTAssertEqual(app.sheets.count, 1)
        XCTAssertFalse(app.buttons["show-catalog-in-finder"].isHittable)
        app.typeKey(.escape, modifierFlags: [])

        let retry = app.buttons["retry-deferred-recovery"]
        XCTAssertTrue(retry.waitForExistence(timeout: 5))
        retry.click()
        XCTAssertTrue(app.sheets.firstMatch.waitForExistence(timeout: 5))
        XCTAssertTrue(
            app.sheets.firstMatch.staticTexts["recovery-title"]
                .waitForExistence(timeout: 5)
        )
        app.typeKey(.escape, modifierFlags: [])

        XCTAssertTrue(retry.waitForExistence(timeout: 5))
        expectation(
            for: NSPredicate(format: "hasKeyboardFocus == true"),
            evaluatedWith: retry
        )
        waitForExpectations(timeout: 3)
    }

    @MainActor
    func testFinishesDetectedRecovery() throws {
        let app = launch(scenario: "import-recovery")
        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()
        let sheet = activeSheet(app)
        XCTAssertTrue(sheet.staticTexts["recovery-title"].waitForExistence(timeout: 5))
        sheet.buttons["finish-recovery"].click()

        XCTAssertTrue(app.staticTexts["import-completed"].waitForExistence(timeout: 5))
    }

    @MainActor
    private func openImportReview(_ app: XCUIApplication) -> XCUIElement {
        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        app.buttons["open-catalog"].click()
        XCTAssertTrue(app.staticTexts["catalog-ready"].waitForExistence(timeout: 5))
        app.buttons["choose-import-folder"].click()
        let sheet = activeSheet(app)
        XCTAssertTrue(
            sheet.descendants(matching: .any)["import-review-sheet"]
                .waitForExistence(timeout: 5)
        )
        return sheet
    }

    @MainActor
    private func activeSheet(_ app: XCUIApplication) -> XCUIElement {
        let sheet = app.sheets.firstMatch
        XCTAssertTrue(sheet.waitForExistence(timeout: 5))
        return sheet
    }

    @MainActor
    private func launch(scenario: String) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = [
            "-ApplePersistenceIgnoreState", "YES",
            "-NSQuitAlwaysKeepsWindows", "NO",
            "-AppleKeyboardUIMode", "3"
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
