import XCTest
@testable import Souvenirs

@MainActor
final class CatalogWelcomeModelTests: XCTestCase {
    func testCatalogActionsRequireReadyEngine() {
        let connecting = EngineAppModel.Status.connecting
        let ready = EngineAppModel.Status.ready(engineVersion: "0.1.0")
        let failed = EngineAppModel.Status.failed("Could not start.")

        XCTAssertFalse(connecting.catalogActionsAvailable)
        XCTAssertEqual(
            connecting.catalogActionsUnavailableDescription,
            "Souvenirs is still connecting to its engine."
        )
        XCTAssertTrue(ready.catalogActionsAvailable)
        XCTAssertNil(ready.catalogActionsUnavailableDescription)
        XCTAssertFalse(failed.catalogActionsAvailable)
        XCTAssertEqual(
            failed.catalogActionsUnavailableDescription,
            "The Souvenirs engine is unavailable. Could not start."
        )
    }

    func testCreateNavigationValidationAndRecentCatalog() async {
        let engine = FakeCatalogEngine()
        let model = CatalogWelcomeModel(
            engine: engine,
            sourceDiscovery: StaticSourceDiscovery(),
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
                    name: "Family Archive",
                    volumeName: "photos-nas.local"
                )
            )
        )
        XCTAssertEqual(model.recents.map(\.path), ["/Volumes/photos/Family"])
    }

    func testCancelCreatePreservesWelcomeRecents() {
        let recent = RecentCatalog(
            path: "/Volumes/photos/Existing",
            name: "Existing",
            volumeName: "photos"
        )
        let model = CatalogWelcomeModel(
            engine: FakeCatalogEngine(),
            sourceDiscovery: StaticSourceDiscovery(),
            defaults: isolatedDefaults(),
            recents: [recent]
        )

        model.beginCreate()
        model.cancelCreate()

        XCTAssertEqual(model.route, .welcome)
        XCTAssertEqual(model.recents, [recent])
        XCTAssertNil(model.selectedPath)
        XCTAssertEqual(model.validation, .none)
    }

    func testCreateFailureKeepsSheetStateAndShowsReason() async {
        let engine = FakeCatalogEngine()
        let model = CatalogWelcomeModel(
            engine: engine,
            sourceDiscovery: StaticSourceDiscovery(),
            defaults: isolatedDefaults(),
            recents: []
        )

        model.beginCreate()
        await model.chooseCreateLocation("/Volumes/photos/Family")
        engine.createError = TestError(message: "The archive could not be created.")
        await model.createCatalog()

        XCTAssertEqual(model.route, .create)
        XCTAssertEqual(model.presentedErrorTitle, "Souvenirs couldn’t create this archive")
        XCTAssertEqual(model.presentedError, "The archive could not be created.")
        XCTAssertTrue(model.canCreate)
    }

    func testValidationErrorBlocksCreation() async {
        let engine = FakeCatalogEngine()
        engine.error = EngineSupervisor.SupervisorError.protocolError(
            code: "synchronized_folder",
            message: "Synchronized folders are unsupported."
        )
        let model = CatalogWelcomeModel(
            engine: engine,
            sourceDiscovery: StaticSourceDiscovery(),
            defaults: isolatedDefaults(),
            recents: []
        )

        model.beginCreate()
        await model.chooseCreateLocation("/Users/test/Dropbox/Family")

        XCTAssertEqual(
            model.validation,
            .invalid(
                """
                This synchronized folder cannot hold an archive because it may rename files on its own.
                """
            )
        )
        XCTAssertFalse(model.canCreate)
    }

    func testWriteValidationErrorShowsReasonAndBlocksCreation() async {
        let engine = FakeCatalogEngine()
        engine.error = EngineSupervisor.SupervisorError.protocolError(
            code: "catalog_io",
            message: """
                Souvenirs cannot write to /Volumes/Archive. \
                Check that the folder or volume allows changes.
                """
        )
        let model = CatalogWelcomeModel(
            engine: engine,
            sourceDiscovery: StaticSourceDiscovery(),
            defaults: isolatedDefaults(),
            recents: []
        )

        model.beginCreate()
        await model.chooseCreateLocation("/Volumes/Archive")

        XCTAssertEqual(
            model.validation,
            .invalid(
                """
                Souvenirs couldn’t access this folder. Check that it’s available and allows changes, \
                then try again.
                """
            )
        )
        XCTAssertFalse(model.canCreate)
    }

    func testOpenErrorAndRecentRemoval() async {
        let engine = FakeCatalogEngine()
        engine.error = EngineSupervisor.SupervisorError.protocolError(
            code: "not_a_catalog",
            message: "This folder is not a Souvenirs catalog."
        )
        let recent = RecentCatalog(
            path: "/Volumes/photos/Family",
            name: "Family",
            volumeName: "photos"
        )
        let model = CatalogWelcomeModel(
            engine: engine,
            sourceDiscovery: StaticSourceDiscovery(),
            defaults: isolatedDefaults(),
            recents: [recent]
        )

        await model.openCatalog(at: recent.path, name: recent.name)
        XCTAssertEqual(model.route, .welcome)
        XCTAssertEqual(model.presentedErrorTitle, "Souvenirs couldn’t open this archive")
        XCTAssertEqual(
            model.presentedError,
            "This folder is not a Souvenirs archive. Choose a folder containing .souvenirs.yml."
        )

        model.removeRecent(path: recent.path)
        XCTAssertTrue(model.recents.isEmpty)
    }

    func testOpeningRecentMovesItToFrontWithoutDuplication() async {
        let engine = FakeCatalogEngine()
        let first = RecentCatalog(path: "/one", name: "One", volumeName: "disk")
        let second = RecentCatalog(path: "/two", name: "Two", volumeName: "disk")
        let model = CatalogWelcomeModel(
            engine: engine,
            sourceDiscovery: StaticSourceDiscovery(),
            defaults: isolatedDefaults(),
            recents: [first, second]
        )

        await model.openCatalog(at: second.path, name: second.name)

        XCTAssertEqual(model.recents.map(\.path), ["/two", "/one"])
    }

    func testSourceRefreshPreservesSelectionThenFallsBackDeterministically() async {
        let discovery = FakeCatalogSourceDiscovery(
            responses: [
                .success([.camera, .backup]),
                .success([.backup, .other]),
                .success([.other])
            ]
        )
        let model = CatalogWelcomeModel(
            engine: FakeCatalogEngine(),
            sourceDiscovery: discovery,
            defaults: isolatedDefaults(),
            recents: []
        )

        await model.openCatalog(at: "/catalog")
        await waitUntil { model.connectedSources.count == 2 }
        model.selectConnectedSource(id: ConnectedSource.backup.id)

        discovery.emitChange()
        await waitUntil { model.connectedSources.contains(.other) }
        XCTAssertEqual(model.selectedConnectedSource, .backup)

        discovery.emitChange()
        await waitUntil { model.connectedSources == [.other] }
        XCTAssertEqual(model.selectedConnectedSource, .other)
    }

    func testDiscoveryFailureClearsPreviouslyConnectedSourceAndRetries() async {
        let discovery = FakeCatalogSourceDiscovery(
            responses: [
                .success([.camera]),
                .failure(SourceDiscoveryError.enumerationUnavailable),
                .success([.camera])
            ]
        )
        let model = CatalogWelcomeModel(
            engine: FakeCatalogEngine(),
            sourceDiscovery: discovery,
            defaults: isolatedDefaults(),
            recents: []
        )

        await model.openCatalog(at: "/catalog")
        await waitUntil { model.selectedConnectedSource == .camera }

        discovery.emitChange()
        await waitUntil { model.sourceDiscoveryError != nil }
        XCTAssertTrue(model.connectedSources.isEmpty)
        XCTAssertNil(model.selectedConnectedSource)

        model.retrySourceDiscovery()
        await waitUntil { model.selectedConnectedSource == .camera }
        XCTAssertNil(model.sourceDiscoveryError)
    }

    func testSupersededDiscoveryCannotRestoreStaleSources() async {
        let discovery = FakeCatalogSourceDiscovery(suspendsDiscoveries: true)
        let model = CatalogWelcomeModel(
            engine: FakeCatalogEngine(),
            sourceDiscovery: discovery,
            defaults: isolatedDefaults(),
            recents: []
        )

        await model.openCatalog(at: "/catalog")
        await waitUntil { discovery.pendingCount == 1 }
        model.refreshConnectedSources()
        await waitUntil { discovery.pendingCount == 2 }

        discovery.resumeDiscovery(at: 1, with: .success([.backup]))
        await waitUntil { model.selectedConnectedSource == .backup }
        discovery.resumeDiscovery(at: 0, with: .success([.camera]))
        await Task.yield()

        XCTAssertEqual(model.connectedSources, [.backup])
        XCTAssertEqual(model.selectedConnectedSource, .backup)
    }

    func testCatalogSwitchIgnoresPreviousLifecycleCompletion() async {
        let discovery = FakeCatalogSourceDiscovery(suspendsDiscoveries: true)
        let model = CatalogWelcomeModel(
            engine: FakeCatalogEngine(),
            sourceDiscovery: discovery,
            defaults: isolatedDefaults(),
            recents: []
        )

        await model.openCatalog(at: "/first")
        await waitUntil { discovery.pendingCount == 1 }
        await model.openCatalog(at: "/second")
        await waitUntil { discovery.pendingCount == 2 }

        discovery.resumeDiscovery(at: 0, with: .success([.camera]))
        await Task.yield()
        XCTAssertTrue(model.connectedSources.isEmpty)

        discovery.resumeDiscovery(at: 0, with: .success([.other]))
        await waitUntil { model.selectedConnectedSource == .other }
        XCTAssertEqual(discovery.stopCount, 2)
    }

    func testReviewValidationRetriesAfterSupersededRefresh() async {
        let discovery = FakeCatalogSourceDiscovery(suspendsDiscoveries: true)
        let model = CatalogWelcomeModel(
            engine: FakeCatalogEngine(),
            sourceDiscovery: discovery,
            defaults: isolatedDefaults(),
            recents: []
        )

        await model.openCatalog(at: "/catalog")
        await waitUntil { discovery.pendingCount == 1 }
        discovery.resumeDiscovery(at: 0, with: .success([.camera]))
        await waitUntil { model.selectedConnectedSource == .camera }

        let review = Task {
            await model.reviewConnectedSource(
                id: ConnectedSource.camera.id,
                into: RecentCatalog(
                    path: "/catalog",
                    name: "Archive",
                    volumeName: "disk"
                )
            )
        }
        await waitUntil { discovery.pendingCount == 1 }
        discovery.emitChange()
        await waitUntil { discovery.pendingCount == 2 }

        discovery.resumeDiscovery(at: 0, with: .success([.camera]))
        await waitUntil { discovery.pendingCount == 2 }
        discovery.resumeDiscovery(at: 1, with: .success([.camera]))

        let started = await review.value
        XCTAssertTrue(started)
        XCTAssertNotNil(model.importPlanning)
        discovery.resumeDiscovery(at: 0, with: .success([.camera]))
    }

    func testReviewValidationDoesNotOpenSheetAfterDisconnect() async {
        let discovery = FakeCatalogSourceDiscovery(
            responses: [
                .success([.camera]),
                .success([])
            ]
        )
        let model = CatalogWelcomeModel(
            engine: FakeCatalogEngine(),
            sourceDiscovery: discovery,
            defaults: isolatedDefaults(),
            recents: []
        )

        await model.openCatalog(at: "/catalog")
        await waitUntil { model.selectedConnectedSource == .camera }

        let started = await model.reviewConnectedSource(
            id: ConnectedSource.camera.id,
            into: RecentCatalog(path: "/catalog", name: "Archive", volumeName: "disk")
        )

        XCTAssertFalse(started)
        XCTAssertNil(model.importPlanning)
        XCTAssertTrue(model.connectedSources.isEmpty)
    }

    func testCatalogProtocolErrorsUseArchiveTerminologyWithoutEchoingInternalPaths() {
        XCTAssertEqual(
            archiveUserMessage(
                code: "catalog_io",
                message: "Could not inspect /Volumes/photos/media catalog/2026: catalog is busy."
            ),
            """
            Souvenirs couldn’t access this folder. Check that it’s available and allows changes, \
            then try again.
            """
        )
        XCTAssertEqual(
            archiveUserMessage(
                code: "destination_preflight_failed",
                message: "catalog mutations are not supported in synchronized folder /Volumes/share"
            ),
            """
            Souvenirs couldn’t complete this archive operation. Check the selected folders and \
            volumes, then try again.
            """
        )
        XCTAssertEqual(
            archiveUserMessage(
                code: "source_io",
                message: "/Volumes/photos/Family Catalog: No space left on device"
            ),
            "/Volumes/photos/Family Catalog: No space left on device"
        )
    }

    private func isolatedDefaults() -> UserDefaults {
        let name = "CatalogWelcomeModelTests-\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: name)!
        defaults.removePersistentDomain(forName: name)
        return defaults
    }

    private func waitUntil(
        _ condition: @escaping @MainActor () -> Bool
    ) async {
        for _ in 0..<100 {
            if condition() { return }
            await Task.yield()
        }
        XCTFail("Condition was not met")
    }
}

@MainActor
private final class FakeCatalogEngine: CatalogEngineClient {
    var error: Error?
    var createError: Error?

    func validateCatalog(path: String, intent: String) async throws
        -> EngineSupervisor.CatalogLocation {
        try result(path)
    }

    func createCatalog(
        path: String,
        displayName: String
    ) async throws -> EngineSupervisor.CatalogLocation {
        if let createError { throw createError }
        return try result(path)
    }

    func openCatalog(path: String) async throws -> EngineSupervisor.CatalogLocation {
        try result(path)
    }

    func catalogCollections(
        path: String
    ) async throws -> [EngineSupervisor.CatalogCollection] {
        throw TestError(message: "Not implemented in catalog tests.")
    }

    func catalogBrowse(
        path: String,
        selection: EngineSupervisor.CatalogCollectionIdentity?
    ) async throws -> EngineSupervisor.CatalogBrowseSnapshot {
        .init(navigation: [.allPhotos], collections: [])
    }

    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan {
        throw TestError(message: "Not implemented in catalog tests.")
    }

    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        destination: EngineSupervisor.ImportDestination,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview {
        throw TestError(message: "Not implemented in catalog tests.")
    }

    private func result(_ path: String) throws -> EngineSupervisor.CatalogLocation {
        if let error { throw error }
        return .init(
            path: path,
            volumeName: "photos-nas.local",
            displayName: "Family Archive",
            detail: "Catalog format 2"
        )
    }
}

private struct TestError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

@MainActor
private final class StaticSourceDiscovery: SourceDiscoveryClient {
    func discoverSources(catalogPath: String) async throws -> [ConnectedSource] {
        []
    }

    func startObserving(_ onChange: @escaping @MainActor () -> Void) {}

    func stopObserving() {}
}

@MainActor
private final class FakeCatalogSourceDiscovery: SourceDiscoveryClient {
    typealias Response = Result<[ConnectedSource], Error>

    private var responses: [Response]
    private let suspendsDiscoveries: Bool
    private var pending: [CheckedContinuation<[ConnectedSource], Error>] = []
    private var onChange: (@MainActor () -> Void)?
    private(set) var stopCount = 0

    init(
        responses: [Response] = [],
        suspendsDiscoveries: Bool = false
    ) {
        self.responses = responses
        self.suspendsDiscoveries = suspendsDiscoveries
    }

    var pendingCount: Int {
        pending.count
    }

    func discoverSources(catalogPath: String) async throws -> [ConnectedSource] {
        if suspendsDiscoveries {
            return try await withCheckedThrowingContinuation {
                pending.append($0)
            }
        }
        guard !responses.isEmpty else { return [] }
        return try responses.removeFirst().get()
    }

    func startObserving(_ onChange: @escaping @MainActor () -> Void) {
        self.onChange = onChange
    }

    func stopObserving() {
        stopCount += 1
        onChange = nil
    }

    func emitChange() {
        onChange?()
    }

    func resumeDiscovery(at index: Int, with response: Response) {
        pending.remove(at: index).resume(with: response)
    }
}

private extension ConnectedSource {
    static let camera = ConnectedSource(
        id: "uuid:camera",
        rootPath: "/Volumes/EOS_DIGITAL",
        name: "EOS_DIGITAL"
    )
    static let backup = ConnectedSource(
        id: "uuid:backup",
        rootPath: "/Volumes/BACKUP_CARD",
        name: "BACKUP_CARD"
    )
    static let other = ConnectedSource(
        id: "uuid:other",
        rootPath: "/Volumes/OTHER",
        name: "OTHER"
    )
}
