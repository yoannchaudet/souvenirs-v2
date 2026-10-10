import XCTest

final class SouvenirsUITests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    @MainActor
    func testAppLaunches() throws {
        let app = XCUIApplication()
        app.launchArguments = ["-ApplePersistenceIgnoreState", "YES"]
        app.launch()

        XCTAssertTrue(app.staticTexts["souvenirs-welcome"].waitForExistence(timeout: 5))
        let readyStatus = app.staticTexts.matching(
            NSPredicate(
                format: "identifier == %@ AND value == %@",
                "engine-status",
                "Engine ready"
            )
        ).firstMatch
        XCTAssertTrue(
            readyStatus.waitForExistence(timeout: 10),
            "The engine did not reach the ready state."
        )
    }
}
