import XCTest
@testable import Souvenirs

@MainActor
final class CatalogBrowseModelTests: XCTestCase {
    func testLoadsOverviewThenExactSelectedCollection() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview, ongoingOverview]
        engine.details[datedIdentity] = datedDetail
        let model = CatalogBrowseModel(engine: engine)

        await model.load(catalogPath: "/catalog")

        XCTAssertEqual(engine.requests, [nil])
        XCTAssertEqual(model.collections.map(\.identity), [datedIdentity, ongoingIdentity])
        XCTAssertEqual(model.selection, .overview)

        await model.select(.collection(datedIdentity))

        XCTAssertEqual(engine.requests, [nil, datedIdentity])
        XCTAssertEqual(model.detail?.identity, datedIdentity)
        XCTAssertFalse(model.isLoadingDetail)
    }

    func testCompleteFailureClearsPreviouslyLoadedSnapshot() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview]
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
        XCTAssertEqual(problemCollection.browseProblem, "Malformed manifest.")
    }

    func testLateDetailResponseCannotReplaceNewerSelection() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview, ongoingOverview]
        engine.details[datedIdentity] = datedDetail
        engine.details[ongoingIdentity] = ongoingDetail
        engine.suspendedIdentity = datedIdentity
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")

        let stale = Task { await model.select(.collection(datedIdentity)) }
        await engine.waitUntilSuspended()
        await model.select(.collection(ongoingIdentity))
        engine.resumeSuspended()
        await stale.value

        XCTAssertEqual(model.selection, .collection(ongoingIdentity))
        XCTAssertEqual(model.detail?.identity, ongoingIdentity)
    }

    func testSelectionIsIgnoredWhileOverviewLoads() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview]
        engine.suspendOverview = true
        let model = CatalogBrowseModel(engine: engine)

        let load = Task { await model.load(catalogPath: "/catalog") }
        await engine.waitUntilSuspended()
        await model.select(.collection(datedIdentity))

        XCTAssertEqual(model.selection, .overview)
        engine.resumeSuspended()
        await load.value
        XCTAssertFalse(model.isLoadingOverview)
        XCTAssertEqual(model.collections.map(\.identity), [datedIdentity])
    }

    func testMissingScopedIdentityRefreshesOverview() async {
        let engine = FakeBrowseEngine()
        engine.overview = [datedOverview]
        let model = CatalogBrowseModel(engine: engine)
        await model.load(catalogPath: "/catalog")

        await model.select(.collection(datedIdentity))

        XCTAssertEqual(engine.requests, [nil, datedIdentity, nil])
        XCTAssertEqual(model.selection, .overview)
        XCTAssertEqual(model.collections.map(\.identity), [datedIdentity])
    }

    private var datedIdentity: EngineSupervisor.CatalogCollectionIdentity {
        .dated(year: "2019", directory: "2019-07-14 +6d Brittany")
    }

    private var ongoingIdentity: EngineSupervisor.CatalogCollectionIdentity {
        .ongoing(key: "kids")
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

    private func media(_ filename: String) -> EngineSupervisor.CatalogBrowseMedia {
        .init(filename: filename, path: "/catalog/\(filename)")
    }
}

@MainActor
private final class FakeBrowseEngine: CatalogEngineClient {
    var overview: [EngineSupervisor.CatalogBrowseCollection] = []
    var details:
        [EngineSupervisor.CatalogCollectionIdentity: EngineSupervisor.CatalogBrowseCollection] = [:]
    var error: Error?
    var suspendedIdentity: EngineSupervisor.CatalogCollectionIdentity?
    var suspendOverview = false
    private(set) var requests: [EngineSupervisor.CatalogCollectionIdentity?] = []
    private var continuation:
        CheckedContinuation<[EngineSupervisor.CatalogBrowseCollection], Never>?

    func catalogBrowse(
        path: String,
        selection: EngineSupervisor.CatalogCollectionIdentity?
    ) async throws -> [EngineSupervisor.CatalogBrowseCollection] {
        requests.append(selection)
        if let error { throw error }
        if (suspendOverview && selection == nil)
            || suspendedIdentity.map({ selection == $0 }) == true {
            return await withCheckedContinuation { continuation = $0 }
        }
        guard let selection else { return overview }
        return details[selection].map { [$0] } ?? []
    }

    func waitUntilSuspended() async {
        while continuation == nil {
            await Task.yield()
        }
    }

    func resumeSuspended() {
        let response: [EngineSupervisor.CatalogBrowseCollection]
        if suspendOverview {
            suspendOverview = false
            response = overview
        } else if let suspendedIdentity {
            response = details[suspendedIdentity].map { [$0] } ?? []
        } else {
            response = []
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
