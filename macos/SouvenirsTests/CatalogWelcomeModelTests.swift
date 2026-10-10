import XCTest
@testable import Souvenirs

@MainActor
final class CatalogWelcomeModelTests: XCTestCase {
    func testCreateNavigationValidationAndRecentCatalog() async {
        let engine = FakeCatalogEngine()
        let model = CatalogWelcomeModel(
            engine: engine,
            defaults: isolatedDefaults(),
            recents: []
        )

        model.beginCreate()
        XCTAssertEqual(model.route, .create)
        XCTAssertFalse(model.canCreate)

        await model.chooseCreateLocation("/Volumes/photos/Family")
        XCTAssertEqual(model.validation, .valid(volumeName: "photos-nas.local"))
        XCTAssertTrue(model.canCreate)

        await model.createCatalog()
        XCTAssertEqual(
            model.route,
            .catalog(
                RecentCatalog(
                    path: "/Volumes/photos/Family",
                    name: "Family Catalog",
                    volumeName: "photos-nas.local"
                )
            )
        )
        XCTAssertEqual(model.recents.map(\.path), ["/Volumes/photos/Family"])
    }

    func testValidationErrorBlocksCreation() async {
        let engine = FakeCatalogEngine()
        engine.error = EngineSupervisor.SupervisorError.protocolError(
            code: "synchronized_folder",
            message: "Synchronized folders are unsupported."
        )
        let model = CatalogWelcomeModel(
            engine: engine,
            defaults: isolatedDefaults(),
            recents: []
        )

        model.beginCreate()
        await model.chooseCreateLocation("/Users/test/Dropbox/Family")

        XCTAssertEqual(
            model.validation,
            .invalid("Synchronized folders are unsupported.")
        )
        XCTAssertFalse(model.canCreate)
    }

    func testOpenErrorAndRecentRemoval() async {
        let engine = FakeCatalogEngine()
        engine.error = TestError(message: "Not a catalog.")
        let recent = RecentCatalog(
            path: "/Volumes/photos/Family",
            name: "Family",
            volumeName: "photos"
        )
        let model = CatalogWelcomeModel(
            engine: engine,
            defaults: isolatedDefaults(),
            recents: [recent]
        )

        await model.openCatalog(at: recent.path, name: recent.name)
        XCTAssertEqual(model.route, .welcome)
        XCTAssertEqual(model.presentedError, "Not a catalog.")

        model.removeRecent(path: recent.path)
        XCTAssertTrue(model.recents.isEmpty)
    }

    func testOpeningRecentMovesItToFrontWithoutDuplication() async {
        let engine = FakeCatalogEngine()
        let first = RecentCatalog(path: "/one", name: "One", volumeName: "disk")
        let second = RecentCatalog(path: "/two", name: "Two", volumeName: "disk")
        let model = CatalogWelcomeModel(
            engine: engine,
            defaults: isolatedDefaults(),
            recents: [first, second]
        )

        await model.openCatalog(at: second.path, name: second.name)

        XCTAssertEqual(model.recents.map(\.path), ["/two", "/one"])
    }

    private func isolatedDefaults() -> UserDefaults {
        let name = "CatalogWelcomeModelTests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: name)!
        defaults.removePersistentDomain(forName: name)
        return defaults
    }
}

@MainActor
private final class FakeCatalogEngine: CatalogEngineClient {
    var error: Error?

    func validateCatalog(path: String, intent: String) async throws
        -> EngineSupervisor.CatalogLocation {
        try result(path)
    }

    func createCatalog(
        path: String,
        displayName: String
    ) async throws -> EngineSupervisor.CatalogLocation {
        try result(path)
    }

    func openCatalog(path: String) async throws -> EngineSupervisor.CatalogLocation {
        try result(path)
    }

    private func result(_ path: String) throws -> EngineSupervisor.CatalogLocation {
        if let error { throw error }
        return .init(
            path: path,
            volumeName: "photos-nas.local",
            displayName: "Family Catalog",
            detail: "Catalog format 2"
        )
    }
}

private struct TestError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}
