import XCTest
@testable import Souvenirs

@MainActor
final class ImportPlanningModelTests: XCTestCase {
    func testLoadsScanAndCanStartWithoutOptionalDateDecision() async {
        let engine = FakeImportPlanningEngine()
        let model = makeModel(engine: engine)

        await model.load()

        XCTAssertEqual(model.scan?.mediaCount, 12)
        XCTAssertEqual(model.collectionName, "Dolomites")
        XCTAssertEqual(model.acceptedDate, "2026-10-10")
        XCTAssertEqual(model.review?.requiredDecisionCount, 1)
        XCTAssertEqual(model.dateChoiceCount, 4)
        XCTAssertTrue(model.supportsUTCConversion)
        XCTAssertTrue(model.canStart)
        XCTAssertEqual(model.review?.transferCount, 8)
        XCTAssertEqual(model.review?.skippedCount, 4)

        await model.resolveDate(.asShown)

        XCTAssertEqual(model.review?.dateResolution, .asShown)
        XCTAssertEqual(model.review?.transferCount, 12)
        XCTAssertTrue(model.canStart)
        await model.changeDateResolution()
        XCTAssertNil(model.dateResolution)
        XCTAssertTrue(model.canStart)
        model.prepareExecution()
        XCTAssertTrue(model.isPrepared)
    }
    func testCollectionEditInvalidatesPlanUntilRevalidated() async {
        let engine = FakeImportPlanningEngine()
        let model = makeModel(engine: engine)
        await model.load()
        await model.resolveDate(.asShown)
        XCTAssertTrue(model.canStart)

        model.collectionName = "Family Trip"
        model.collectionNameChanged()

        XCTAssertFalse(model.canStart)
        XCTAssertNil(model.review)
        await model.refreshReview()
        XCTAssertEqual(engine.lastCollectionName, "Family Trip")
        XCTAssertTrue(model.canStart)
    }
    func testDestinationChangeRebuildsReview() async {
        let engine = FakeImportPlanningEngine()
        let model = makeModel(engine: engine)
        await model.load()

        await model.loadCollections()
        guard let collection = model.availableCollections.first else {
            return XCTFail("Expected a catalog collection")
        }
        await model.selectExistingCollection(collection)

        XCTAssertEqual(engine.lastDestinationMode, .existing)
        XCTAssertEqual(
            model.review?.destination,
            .existing(identity: collection.identity)
        )
    }

    func testOngoingSelectionUsesExactIdentityAndCreateRestoresName() async {
        let engine = FakeImportPlanningEngine()
        let model = makeModel(engine: engine)
        await model.load()
        model.collectionName = "Family Trip"
        model.collectionNameChanged()
        await model.loadCollections()
        guard let ongoing = model.availableCollections.last else {
            return XCTFail("Expected an ongoing collection")
        }

        await model.selectExistingCollection(ongoing)

        XCTAssertEqual(
            engine.lastDestination,
            .existing(identity: .ongoing(key: "kids"))
        )
        XCTAssertEqual(model.collectionName, "Kids")
        XCTAssertEqual(model.selectedExistingCollection, ongoing)

        await model.useCreateDestination()

        XCTAssertEqual(model.collectionName, "Family Trip")
        XCTAssertNil(model.selectedExistingCollection)
        XCTAssertEqual(
            engine.lastDestination,
            .create(collectionName: "Family Trip")
        )
    }

    func testUnavailableDestinationPreservesSelectedIdentity() async {
        let engine = FakeImportPlanningEngine()
        let model = makeModel(engine: engine)
        await model.load()
        await model.loadCollections()
        engine.reviewError = EngineSupervisor.SupervisorError.protocolError(
            code: "destination_unavailable",
            message: "The selected collection is no longer available. Choose it again."
        )
        guard let collection = model.availableCollections.first else {
            return XCTFail("Expected a catalog collection")
        }

        await model.selectExistingCollection(collection)

        XCTAssertEqual(model.selectedExistingCollection, collection)
        XCTAssertEqual(model.destination, .existing(identity: collection.identity))
        XCTAssertEqual(model.errorCode, "destination_unavailable")
        XCTAssertFalse(model.canStart)
    }

    func testScanFailureIsActionableAndBlocksStart() async {
        let engine = FakeImportPlanningEngine()
        engine.error = EngineSupervisor.SupervisorError.protocolError(
            code: "source_unavailable",
            message: "Reconnect the source and try again."
        )
        let model = makeModel(engine: engine)

        await model.load()

        XCTAssertEqual(model.errorMessage, "Reconnect the source and try again.")
        XCTAssertEqual(model.errorCode, "source_unavailable")
        XCTAssertEqual(model.title, "This source is unavailable")
        XCTAssertTrue(model.canRetryScan)
        XCTAssertNil(model.scan)
        XCTAssertFalse(model.canStart)
    }

    func testRetryClearsFailureAndScansTheSameSourceOncePerActivation() async {
        let engine = FakeImportPlanningEngine()
        engine.scanErrors = [
            EngineSupervisor.SupervisorError.protocolError(
                code: "source_unavailable",
                message: "Reconnect the source and try again."
            )
        ]
        let model = makeModel(engine: engine)

        await model.load()
        XCTAssertTrue(model.canRetryScan)
        XCTAssertEqual(engine.scannedSourcePaths, ["/Volumes/EOS_DIGITAL"])

        engine.pauseNextScan = true
        let retry = Task { await model.retryScan() }
        await Task.yield()

        XCTAssertTrue(model.isLoading)
        XCTAssertNil(model.errorMessage)
        XCTAssertNil(model.errorCode)
        XCTAssertFalse(model.canStart)
        XCTAssertEqual(
            engine.scannedSourcePaths,
            ["/Volumes/EOS_DIGITAL", "/Volumes/EOS_DIGITAL"]
        )

        engine.resumeScan()
        await retry.value

        XCTAssertNil(model.errorMessage)
        XCTAssertNil(model.errorCode)
        XCTAssertEqual(model.sourcePath, "/Volumes/EOS_DIGITAL")
        XCTAssertEqual(
            engine.scannedSourcePaths,
            ["/Volumes/EOS_DIGITAL", "/Volumes/EOS_DIGITAL"]
        )
        XCTAssertEqual(model.scan?.sourceName, "EOS_DIGITAL")
        XCTAssertEqual(model.collectionName, "Dolomites")
        XCTAssertTrue(model.canStart)

        await model.retryScan()
        XCTAssertEqual(engine.scannedSourcePaths.count, 2)
    }

    func testFailedRetryRemainsRetryableAndBlocked() async {
        let engine = FakeImportPlanningEngine()
        engine.scanErrors = [
            EngineSupervisor.SupervisorError.protocolError(
                code: "source_unavailable",
                message: "Reconnect the source and try again."
            ),
            EngineSupervisor.SupervisorError.protocolError(
                code: "source_unavailable",
                message: "The source is still unavailable."
            )
        ]
        let model = makeModel(engine: engine)

        await model.load()
        await model.retryScan()

        XCTAssertEqual(model.errorMessage, "The source is still unavailable.")
        XCTAssertEqual(model.title, "This source is unavailable")
        XCTAssertTrue(model.canRetryScan)
        XCTAssertFalse(model.canStart)
        XCTAssertEqual(engine.scannedSourcePaths.count, 2)
    }

    func testCancellationSuppressesLateScanResult() async {
        let engine = FakeImportPlanningEngine()
        engine.pauseNextScan = true
        let model = makeModel(engine: engine)

        let loading = Task { await model.load() }
        await Task.yield()
        XCTAssertTrue(model.isLoading)

        loading.cancel()
        engine.resumeScan()
        await loading.value

        XCTAssertFalse(model.isLoading)
        XCTAssertNil(model.scan)
        XCTAssertNil(model.errorMessage)
    }

    func testClocklessItemsRemainDisclosedWithoutInterpretationButtons() async {
        let engine = FakeImportPlanningEngine()
        engine.includesDeferredDate = true
        let model = makeModel(engine: engine)

        await model.load()

        XCTAssertEqual(model.dateChoiceCount, 4)
        XCTAssertEqual(model.deferredDateCount, 1)
    }

    func testProposalOnlyChoiceDoesNotOfferUTCConversion() async {
        let engine = FakeImportPlanningEngine()
        engine.supportsUTCConversion = false
        let model = makeModel(engine: engine)

        await model.load()

        XCTAssertFalse(model.supportsUTCConversion)
        XCTAssertEqual(
            model.interpretationChoiceSources.first?.asShownTime,
            "12:14"
        )
    }

    func testSummaryUsesEngineOwnedCountsAndDestination() async {
        let engine = FakeImportPlanningEngine()
        let model = makeModel(engine: engine)
        await model.load()
        await model.resolveDate(.convertFromUTC)

        XCTAssertEqual(model.review?.transferCount, 12)
        XCTAssertEqual(model.review?.leftOnSourceCount, 2)
        XCTAssertEqual(model.review?.operationCount, 14)
        XCTAssertTrue(model.review?.destinationPath.contains("Dolomites") == true)
        XCTAssertEqual(
            model.title,
            "12 new photos and videos will be copied into “Dolomites”"
        )
    }

    private func makeModel(engine: FakeImportPlanningEngine) -> ImportPlanningModel {
        ImportPlanningModel(
            engine: engine,
            catalog: RecentCatalog(
                path: "/Volumes/photos/Family Catalog",
                name: "Family Archive",
                volumeName: "photos-nas.local"
            ),
            sourcePath: "/Volumes/EOS_DIGITAL"
        )
    }
}

@MainActor
private final class FakeImportPlanningEngine: CatalogEngineClient {
    var error: Error?
    var reviewError: Error?
    var lastCollectionName: String?
    var lastDestinationMode: EngineSupervisor.ImportDestinationMode?
    var lastDestination: EngineSupervisor.ImportDestination?
    var includesDeferredDate = false
    var supportsUTCConversion = true
    var scanErrors: [Error] = []
    private(set) var scannedSourcePaths: [String] = []
    var pauseNextScan = false
    private var scanContinuation: CheckedContinuation<Void, Never>?

    func catalogCollections(
        path: String
    ) async throws -> [EngineSupervisor.CatalogCollection] {
        [
            .dated(
                identity: .dated(
                    year: "2019",
                    directory: "2019-07-14 +6d Brittany"
                ),
                label: "Brittany",
                startDate: "2019-07-14",
                endDate: "2019-07-20"
            ),
            .ongoing(
                identity: .ongoing(key: "kids"),
                label: "Kids",
                parts: [
                    .init(
                        year: 2019,
                        yearDirectory: "2019",
                        collectionDirectory: "z Kids"
                    )
                ]
            )
        ]
    }

    func catalogBrowse(
        path: String,
        selection: EngineSupervisor.CatalogCollectionIdentity?
    ) async throws -> EngineSupervisor.CatalogBrowseSnapshot {
        .init(navigation: [.allPhotos], collections: [])
    }

    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan {
        scannedSourcePaths.append(sourcePath)
        if !scanErrors.isEmpty {
            throw scanErrors.removeFirst()
        }
        if let error { throw error }
        if pauseNextScan {
            pauseNextScan = false
            await withCheckedContinuation {
                scanContinuation = $0
            }
        }
        return scan(sourcePath: sourcePath)
    }

    func resumeScan() {
        scanContinuation?.resume()
        scanContinuation = nil
    }

    private func scan(sourcePath: String) -> EngineSupervisor.ImportScan {
        return .init(
            sourcePath: sourcePath,
            sourceName: "EOS_DIGITAL",
            fileCount: 17,
            mediaCount: 12,
            photoCount: 8,
            videoCount: 4,
            unsupportedCount: 2,
            uncertainTimeCount: includesDeferredDate ? 5 : 4,
            totalBytes: 41_000_000_000,
            suggestedCollectionName: "Dolomites",
            suggestedAcceptedDate: "2026-10-10",
            suggestedRangeEnd: "2026-10-12",
            pendingItemCount: includesDeferredDate ? 5 : 4,
            unresolvedItemCount: 0,
            datetimeSources: [
                .init(
                    source: .containerCreationTime,
                    state: .needsConfirmation,
                    itemCount: 4,
                    requiresClock: false,
                    supportsUTCConversion: supportsUTCConversion,
                    asShownTime: "12:14",
                    convertedFromUTCTime: supportsUTCConversion ? "14:14" : "12:14"
                )
            ] + (includesDeferredDate
                ? [
                    .init(
                        source: .filenamePattern,
                        state: .needsConfirmation,
                        itemCount: 1,
                        requiresClock: true,
                        supportsUTCConversion: false,
                        asShownTime: nil,
                        convertedFromUTCTime: nil
                    )
                ]
                : [])
        )
    }

    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        destination: EngineSupervisor.ImportDestination,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview {
        if let reviewError { throw reviewError }
        let collectionName: String
        switch destination {
        case .create(let name):
            collectionName = name
        case .existing(.dated):
            collectionName = "Brittany"
        case .existing(.ongoing):
            collectionName = "Kids"
        }
        lastCollectionName = collectionName
        lastDestinationMode = destination.mode
        lastDestination = destination
        let scan = scan(sourcePath: sourcePath)
        let deferred = includesDeferredDate ? 1 : 0
        return .init(
            scan: scan,
            collectionName: collectionName,
            destination: destination,
            destinationPath: "\(catalogPath)/2026/2026-10-10 \(collectionName)",
            acceptedDate: "2026-10-10",
            dateResolution: dateResolution,
            requiredDecisionCount: dateResolution == nil ? 1 : 0,
            transferCount: dateResolution == nil ? 8 - deferred : 12 - deferred,
            skippedCount: dateResolution == nil ? 4 + deferred : deferred,
            leftOnSourceCount: dateResolution == nil ? 6 + deferred : 2 + deferred,
            operationCount: dateResolution == nil ? 10 - deferred : 14 - deferred,
            preflight: .fixture,
            canStart: true,
            reviewedPlan: "reviewed-plan"
        )
    }

    func validateCatalog(
        path: String,
        intent: String
    ) async throws -> EngineSupervisor.CatalogLocation {
        throw ImportTestError.unused
    }

    func createCatalog(
        path: String,
        displayName: String
    ) async throws -> EngineSupervisor.CatalogLocation {
        throw ImportTestError.unused
    }

    func openCatalog(path: String) async throws -> EngineSupervisor.CatalogLocation {
        throw ImportTestError.unused
    }
}

private enum ImportTestError: Error {
    case unused
}
