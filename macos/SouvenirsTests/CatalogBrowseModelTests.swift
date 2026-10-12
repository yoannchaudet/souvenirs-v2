import XCTest
@testable import Souvenirs

@MainActor
final class CatalogBrowseModelTests: XCTestCase {
    func testLoadsOverviewThenExactSelectedCollection() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview, ongoingOverview]
        engine.navigation = navigation
        engine.details[datedIdentity] = datedDetail
        let model = CatalogBrowseModel(engine: engine)

        await model.load(catalogPath: "/catalog")

        XCTAssertEqual(engine.requests, [nil])
        XCTAssertEqual(model.collections.map(\.identity), [datedIdentity, ongoingIdentity])
        XCTAssertEqual(model.selection, .overview)

        await model.select(datedOccurrence)

        XCTAssertEqual(engine.requests, [nil, datedIdentity])
        XCTAssertEqual(model.detail?.identity, datedIdentity)
        XCTAssertFalse(model.isLoadingDetail)
    }

    func testCompleteFailureClearsPreviouslyLoadedSnapshot() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview]
        engine.navigation = [.allPhotos]
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")
        XCTAssertFalse(model.collections.isEmpty)

        engine.error = EngineSupervisor.SupervisorError.protocolError(
            code: "catalog_io",
            message: "The archive volume is unavailable."
        )
        await model.refresh()

        XCTAssertTrue(model.collections.isEmpty)
        XCTAssertNil(model.detail)
        XCTAssertEqual(model.selection, .overview)
        XCTAssertEqual(
            model.errorMessage,
            """
            Souvenirs couldn’t access this folder. Check that it’s available and allows changes, \
            then try again.
            """
        )
    }

    func testProblemSuppressesUntrustedCountsAndCover() {
        let problemCollection = EngineSupervisor.CatalogBrowseCollection.ongoing(
            identity: ongoingIdentity,
            label: "Kids",
            mediaCount: 100,
            parts: [
                .init(
                    year: 2025,
                    yearDirectory: "2025",
                    collectionDirectory: "z Kids",
                    mediaCount: 100,
                    coverMedia: media("untrusted.jpg"),
                    dateGroups: [
                        .init(date: "2025-01-01", media: [media("untrusted.jpg")])
                    ],
                    problem: "Malformed manifest."
                )
            ]
        )

        XCTAssertNil(problemCollection.trustedMediaCount)
        XCTAssertNil(problemCollection.trustedCoverMedia)
        XCTAssertNil(problemCollection.trustedMediaCount(annualPartYear: 2025))
        XCTAssertNil(problemCollection.trustedCoverMedia(annualPartYear: 2025))
        XCTAssertEqual(problemCollection.browseProblem, "Malformed manifest.")
    }

    func testSelectsRustOwnedScopeWithoutAnotherRequest() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview, ongoingOverview]
        engine.navigation = navigation
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")

        model.selectScope(.year(2024))

        XCTAssertEqual(engine.requests, [nil])
        XCTAssertEqual(model.selectedScope, .year(2024))
        XCTAssertEqual(model.overviewGroups.first?.entries, [
            .init(identity: ongoingIdentity, annualPartYear: 2024)
        ])
    }

    func testRefreshPreservesOccurrenceWhenYearMovesUnderEarlier() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview, ongoingOverview]
        engine.navigation = navigation
        engine.details[ongoingIdentity] = ongoingDetail
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")
        model.selectScope(.year(2024))
        await model.select(ongoingOccurrence)

        engine.suspendOverview = true
        let refresh = Task { await model.refresh() }
        await engine.waitUntilSuspended()
        XCTAssertEqual(model.selectedScope, .year(2024))
        XCTAssertEqual(model.selection, .collection(ongoingOccurrence))
        XCTAssertEqual(model.detail?.identity, ongoingIdentity)

        engine.navigation = [
            .allPhotos,
            .earlier(
                throughYear: 2024,
                years: [
                    .init(
                        year: 2024,
                        entries: [
                            .init(identity: ongoingIdentity, annualPartYear: 2024)
                        ]
                    )
                ]
            ),
        ]
        engine.resumeSuspended()
        await refresh.value

        XCTAssertEqual(model.selectedScope, .allPhotos)
        XCTAssertEqual(model.selection, .collection(ongoingOccurrence))
        XCTAssertEqual(model.detail?.identity, ongoingIdentity)
    }

    func testRefreshFallsBackWhenOccurrenceDisappears() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview, ongoingOverview]
        engine.navigation = navigation
        engine.details[ongoingIdentity] = ongoingDetail
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")
        await model.select(ongoingOccurrence)

        engine.overview = [datedOverview]
        engine.navigation = [.allPhotos]
        await model.refresh()

        XCTAssertEqual(model.selectedScope, .allPhotos)
        XCTAssertEqual(model.selection, .overview)
        XCTAssertNil(model.detail)
    }

    func testRefreshKeepsLiveEarlierSelectionWhenBoundaryChanges() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview]
        engine.navigation = [
            .allPhotos,
            .earlier(
                throughYear: 2019,
                years: [.init(
                    year: 2019,
                    entries: [.init(identity: datedIdentity, annualPartYear: nil)]
                )]
            ),
        ]
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")
        model.selectScope(.earlier(throughYear: 2019))

        engine.suspendOverview = true
        let refresh = Task { await model.refresh() }
        await engine.waitUntilSuspended()
        model.selectScope(.allPhotos)
        model.selectScope(.earlier(throughYear: 2019))
        engine.navigation = [
            .allPhotos,
            .earlier(throughYear: 2020, years: [])
        ]
        engine.resumeSuspended()
        await refresh.value

        XCTAssertEqual(model.selectedScope, .earlier(throughYear: 2020))
        XCTAssertEqual(model.selection, .overview)
    }

    func testLateDetailResponseCannotReplaceNewerSelection() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview, ongoingOverview]
        engine.navigation = navigation
        engine.details[datedIdentity] = datedDetail
        engine.details[ongoingIdentity] = ongoingDetail
        engine.suspendedIdentity = datedIdentity
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")

        let stale = Task { await model.select(datedOccurrence) }
        await engine.waitUntilSuspended()
        await model.select(ongoingOccurrence)
        engine.resumeSuspended()
        await stale.value

        XCTAssertEqual(model.selection, .collection(ongoingOccurrence))
        XCTAssertEqual(model.detail?.identity, ongoingIdentity)
    }

    func testSelectionIsIgnoredWhileOverviewLoads() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview]
        engine.navigation = [.allPhotos]
        engine.suspendOverview = true
        let model = CatalogBrowseModel(engine: engine)

        let load = Task { await model.load(catalogPath: "/catalog") }
        await engine.waitUntilSuspended()
        await model.select(datedOccurrence)

        XCTAssertEqual(model.selection, .overview)
        engine.resumeSuspended()
        await load.value
        XCTAssertFalse(model.isLoadingOverview)
        XCTAssertEqual(model.collections.map(\.identity), [datedIdentity])
    }

    func testMissingScopedIdentityRefreshesOverview() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview]
        engine.navigation = [.allPhotos]
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")

        await model.select(datedOccurrence)

        XCTAssertEqual(engine.requests, [nil, datedIdentity, nil])
        XCTAssertEqual(model.selection, .overview)
        XCTAssertEqual(model.collections.map(\.identity), [datedIdentity])
    }

    func testTransientMediaMergesWithDurableDetailByPathInProtocolOrder() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview]
        engine.navigation = [.allPhotos]
        engine.details[datedIdentity] = datedDetail
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")

        let admitted = operationMedia(
            identity: datedIdentity,
            date: "2019-07-15",
            filename: "brittany.jpg"
        )
        let newMedia = operationMedia(
            identity: datedIdentity,
            date: "2019-07-16",
            filename: "second.jpg"
        )
        await model.updateImportPresentation(
            operationID: "operation-1",
            collectionName: "Brittany",
            media: [admitted, newMedia]
        )

        guard case .dated(
            _,
            _,
            _,
            _,
            let count,
            _,
            let groups,
            _
        ) = model.presentedImportCollection else {
            return XCTFail("Expected a dated transient presentation")
        }
        XCTAssertEqual(count, 2)
        XCTAssertEqual(groups.map(\.date), ["2019-07-15", "2019-07-16"])
        XCTAssertEqual(
            groups.flatMap(\.media).map(\.path),
            ["/catalog/brittany.jpg", "/catalog/second.jpg"]
        )
        XCTAssertFalse(groups[0].media[0].isTransient)
        XCTAssertTrue(groups[1].media[0].isTransient)
        model.selectScope(.allPhotos)
        XCTAssertEqual(model.displayTitle, "All Photos")
    }

    func testTerminalRefreshCommitsDurableLandingAndFocusAtomically() async {
        let engine = FakeBrowseEngine()
        engine.navigation = [.allPhotos]
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")
        await model.updateImportPresentation(
            operationID: "operation-1",
            collectionName: "Dolomites",
            media: []
        )
        XCTAssertEqual(model.displayTitle, "Dolomites")
        XCTAssertNil(model.presentedImportCollection)
        let media = operationMedia(
            identity: importedIdentity,
            date: "2026-10-10",
            filename: "imported.jpg"
        )
        await model.updateImportPresentation(
            operationID: "operation-1",
            collectionName: "Dolomites",
            media: [media],
            loadDetail: false
        )
        XCTAssertNotNil(model.presentedImportCollection)

        engine.overview = [importedOverview]
        engine.navigation = [
            .allPhotos,
            .year(
                year: 2026,
                entries: [.init(identity: importedIdentity, annualPartYear: nil)]
            ),
        ]
        engine.details[importedIdentity] = importedDetail
        await model.reconcileTerminalImport()

        XCTAssertEqual(model.importReconciliation, .landed)
        XCTAssertNil(model.presentedImportCollection)
        XCTAssertEqual(model.detail?.identity, importedIdentity)
        XCTAssertEqual(model.selectedScope, .year(2026))
        XCTAssertEqual(
            model.selection,
            .collection(.init(identity: importedIdentity, annualPartYear: nil))
        )
        XCTAssertEqual(model.landingFocusGeneration, 1)
    }

    func testTerminalScopedFailureKeepsTransientAndDoesNotPublishOverview() async {
        let engine = FakeBrowseEngine()
        engine.navigation = [.allPhotos]
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")
        let media = operationMedia(
            identity: importedIdentity,
            date: "2026-10-10",
            filename: "imported.jpg"
        )
        await model.updateImportPresentation(
            operationID: "operation-1",
            collectionName: "Dolomites",
            media: [media]
        )

        engine.overview = [importedOverview]
        engine.navigation = [
            .allPhotos,
            .year(
                year: 2026,
                entries: [.init(identity: importedIdentity, annualPartYear: nil)]
            ),
        ]
        engine.scopedError = EngineSupervisor.SupervisorError.protocolError(
            code: "catalog_io",
            message: "The archive is unavailable."
        )
        await model.reconcileTerminalImport()

        guard case .failed(let message) = model.importReconciliation else {
            return XCTFail("Expected terminal refresh failure")
        }
        XCTAssertTrue(message.contains("access"))
        XCTAssertTrue(model.collections.isEmpty)
        XCTAssertNotNil(model.presentedImportCollection)
        model.clearImportPresentation()
        XCTAssertEqual(model.selection, .overview)
        await model.updateImportPresentation(
            operationID: "operation-1",
            collectionName: "Dolomites",
            media: [media]
        )

        engine.scopedError = nil
        engine.details[importedIdentity] = importedDetail
        await model.reconcileTerminalImport()
        XCTAssertEqual(model.importReconciliation, .landed)
    }

    func testTerminalRefreshSelectsOngoingOccurrenceUnderEarlier() async {
        let engine = FakeBrowseEngine()
        engine.navigation = [.allPhotos]
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")
        let media = operationMedia(
            identity: ongoingIdentity,
            annualPartYear: 2019,
            date: "2019-03-02",
            filename: "kids.jpg"
        )
        await model.updateImportPresentation(
            operationID: "operation-1",
            collectionName: "Kids",
            media: [media]
        )
        engine.overview = [ongoingOverview]
        engine.navigation = [
            .allPhotos,
            .earlier(
                throughYear: 2019,
                years: [
                    .init(
                        year: 2019,
                        entries: [
                            .init(identity: ongoingIdentity, annualPartYear: 2019)
                        ]
                    )
                ]
            ),
        ]
        engine.details[ongoingIdentity] = ongoingImportedDetail

        await model.reconcileTerminalImport()

        XCTAssertEqual(model.selectedScope, .earlier(throughYear: 2019))
        XCTAssertEqual(
            model.selection,
            .collection(.init(identity: ongoingIdentity, annualPartYear: 2019))
        )
    }

    func testStaleTerminalRefreshCannotLandReplacementOperation() async {
        let engine = FakeBrowseEngine()
        engine.navigation = [.allPhotos]
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")
        let first = operationMedia(
            identity: importedIdentity,
            date: "2026-10-10",
            filename: "first.jpg"
        )
        await model.updateImportPresentation(
            operationID: "operation-1",
            collectionName: "Dolomites",
            media: [first]
        )
        engine.suspendOverview = true
        let stale = Task { await model.reconcileTerminalImport() }
        await engine.waitUntilSuspended()

        let replacement = operationMedia(
            identity: importedIdentity,
            date: "2026-10-11",
            filename: "replacement.jpg"
        )
        await model.updateImportPresentation(
            operationID: "operation-2",
            collectionName: "Dolomites",
            media: [replacement]
        )
        engine.resumeSuspended()
        await stale.value

        XCTAssertEqual(model.importReconciliation, .active)
        guard case .dated(_, _, _, _, _, _, let groups, _) =
            model.presentedImportCollection else {
            return XCTFail("Expected replacement transient media")
        }
        XCTAssertEqual(groups.flatMap(\.media).map(\.filename), ["replacement.jpg"])
    }

    private var datedIdentity: EngineSupervisor.CatalogCollectionIdentity {
        .dated(year: "2019", directory: "2019-07-14 +6d Brittany")
    }

    private var ongoingIdentity: EngineSupervisor.CatalogCollectionIdentity {
        .ongoing(key: "kids")
    }

    private var importedIdentity: EngineSupervisor.CatalogCollectionIdentity {
        .dated(year: "2026", directory: "2026-10-10 +2d Dolomites")
    }

    private var datedOccurrence: CatalogBrowseModel.CollectionOccurrence {
        .init(identity: datedIdentity, annualPartYear: nil)
    }

    private var ongoingOccurrence: CatalogBrowseModel.CollectionOccurrence {
        .init(identity: ongoingIdentity, annualPartYear: 2024)
    }

    private var navigation: [EngineSupervisor.CatalogBrowseNavigationScope] {
        [
            .allPhotos,
            .year(
                year: 2024,
                entries: [
                    .init(identity: ongoingIdentity, annualPartYear: 2024)
                ]
            ),
            .earlier(
                throughYear: 2019,
                years: [
                    .init(
                        year: 2019,
                        entries: [
                            .init(identity: datedIdentity, annualPartYear: nil)
                        ]
                    )
                ]
            ),
        ]
    }

    private var datedOverview: EngineSupervisor.CatalogBrowseCollection {
        .dated(
            identity: datedIdentity,
            label: "Brittany",
            startDate: "2019-07-14",
            endDate: "2019-07-20",
            mediaCount: 1,
            coverMedia: media("brittany.jpg"),
            dateGroups: [],
            problem: nil
        )
    }

    private var ongoingOverview: EngineSupervisor.CatalogBrowseCollection {
        .ongoing(
            identity: ongoingIdentity,
            label: "Kids",
            mediaCount: 1,
            parts: [
                .init(
                    year: 2024,
                    yearDirectory: "2024",
                    collectionDirectory: "z Kids",
                    mediaCount: 1,
                    coverMedia: media("kids.jpg"),
                    dateGroups: [],
                    problem: nil
                )
            ]
        )
    }

    private var datedDetail: EngineSupervisor.CatalogBrowseCollection {
        .dated(
            identity: datedIdentity,
            label: "Brittany",
            startDate: "2019-07-14",
            endDate: "2019-07-20",
            mediaCount: 1,
            coverMedia: media("brittany.jpg"),
            dateGroups: [.init(date: "2019-07-15", media: [media("brittany.jpg")])],
            problem: nil
        )
    }

    private var ongoingDetail: EngineSupervisor.CatalogBrowseCollection {
        .ongoing(
            identity: ongoingIdentity,
            label: "Kids",
            mediaCount: 1,
            parts: [
                .init(
                    year: 2024,
                    yearDirectory: "2024",
                    collectionDirectory: "z Kids",
                    mediaCount: 1,
                    coverMedia: media("kids.jpg"),
                    dateGroups: [.init(date: "2024-06-08", media: [media("kids.jpg")])],
                    problem: nil
                )
            ]
        )
    }

    private var ongoingImportedDetail: EngineSupervisor.CatalogBrowseCollection {
        .ongoing(
            identity: ongoingIdentity,
            label: "Kids",
            mediaCount: 1,
            parts: [
                .init(
                    year: 2019,
                    yearDirectory: "2019",
                    collectionDirectory: "z Kids",
                    mediaCount: 1,
                    coverMedia: media("kids.jpg"),
                    dateGroups: [
                        .init(date: "2019-03-02", media: [media("kids.jpg")])
                    ],
                    problem: nil
                )
            ]
        )
    }

    private var importedOverview: EngineSupervisor.CatalogBrowseCollection {
        .dated(
            identity: importedIdentity,
            label: "Dolomites",
            startDate: "2026-10-10",
            endDate: "2026-10-12",
            mediaCount: 1,
            coverMedia: media("imported.jpg"),
            dateGroups: [],
            problem: nil
        )
    }

    private var importedDetail: EngineSupervisor.CatalogBrowseCollection {
        .dated(
            identity: importedIdentity,
            label: "Dolomites",
            startDate: "2026-10-10",
            endDate: "2026-10-12",
            mediaCount: 1,
            coverMedia: media("imported.jpg"),
            dateGroups: [
                .init(date: "2026-10-10", media: [media("imported.jpg")])
            ],
            problem: nil
        )
    }

    private func media(_ filename: String) -> EngineSupervisor.CatalogBrowseMedia {
        .init(filename: filename, path: "/catalog/\(filename)")
    }

    private func operationMedia(
        identity: EngineSupervisor.CatalogCollectionIdentity,
        annualPartYear: Int? = nil,
        date: String,
        filename: String
    ) -> EngineSupervisor.OperationMedia {
        .init(
            identity: identity,
            annualPartYear: annualPartYear,
            acceptedDate: date,
            filename: filename,
            path: "/catalog/\(filename)"
        )
    }
}

@MainActor
private final class FakeBrowseEngine: CatalogEngineClient {
    var navigation: [EngineSupervisor.CatalogBrowseNavigationScope] = []
    var overview: [EngineSupervisor.CatalogBrowseCollection] = []
    var details:
        [EngineSupervisor.CatalogCollectionIdentity: EngineSupervisor.CatalogBrowseCollection] = [:]
    var error: Error?
    var scopedError: Error?
    var suspendedIdentity: EngineSupervisor.CatalogCollectionIdentity?
    var suspendOverview = false
    private(set) var requests: [EngineSupervisor.CatalogCollectionIdentity?] = []
    private var continuation:
        CheckedContinuation<EngineSupervisor.CatalogBrowseSnapshot, Never>?

    func catalogBrowse(
        path: String,
        selection: EngineSupervisor.CatalogCollectionIdentity?
    ) async throws -> EngineSupervisor.CatalogBrowseSnapshot {
        requests.append(selection)
        if let error { throw error }
        if selection != nil, let scopedError { throw scopedError }
        if (suspendOverview && selection == nil)
            || suspendedIdentity.map({ selection == $0 }) == true {
            return await withCheckedContinuation { continuation = $0 }
        }
        guard let selection else {
            return .init(navigation: navigation, collections: overview)
        }
        return .init(
            navigation: [],
            collections: details[selection].map { [$0] } ?? []
        )
    }

    func waitUntilSuspended() async {
        while continuation == nil {
            await Task.yield()
        }
    }

    func resumeSuspended() {
        let response: EngineSupervisor.CatalogBrowseSnapshot
        if suspendOverview {
            suspendOverview = false
            response = .init(navigation: navigation, collections: overview)
        } else if let suspendedIdentity {
            response = .init(
                navigation: [],
                collections: details[suspendedIdentity].map { [$0] } ?? []
            )
        } else {
            response = .init(navigation: [], collections: [])
        }
        continuation?.resume(returning: response)
        continuation = nil
    }

    func validateCatalog(path: String, intent: String) async throws
        -> EngineSupervisor.CatalogLocation {
        throw TestError()
    }

    func createCatalog(path: String, displayName: String) async throws
        -> EngineSupervisor.CatalogLocation {
        throw TestError()
    }

    func openCatalog(path: String) async throws -> EngineSupervisor.CatalogLocation {
        throw TestError()
    }

    func catalogCollections(path: String) async throws
        -> [EngineSupervisor.CatalogCollection] {
        []
    }

    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan {
        throw TestError()
    }

    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        destination: EngineSupervisor.ImportDestination,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview {
        throw TestError()
    }

    private struct TestError: Error {}
}
