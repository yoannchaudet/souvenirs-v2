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
        XCTAssertEqual(model.asShownTime, "12:14")
        XCTAssertEqual(model.convertedFromUTCTime, "14:14")
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

        await model.setDestinationMode(.existing)

        XCTAssertEqual(engine.lastDestinationMode, .existing)
        XCTAssertEqual(model.review?.destinationMode, .existing)
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
        XCTAssertNil(model.scan)
        XCTAssertFalse(model.canStart)
    }

    func testClocklessItemsRemainDisclosedWithoutInterpretationButtons() async {
        let engine = FakeImportPlanningEngine()
        engine.includesDeferredDate = true
        let model = makeModel(engine: engine)

        await model.load()

        XCTAssertEqual(model.dateChoiceCount, 4)
        XCTAssertEqual(model.deferredDateCount, 1)
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
                name: "Family Catalog",
                volumeName: "photos-nas.local"
            ),
            sourcePath: "/Volumes/EOS_DIGITAL"
        )
    }
}

@MainActor
private final class FakeImportPlanningEngine: CatalogEngineClient {
    var error: Error?
    var lastCollectionName: String?
    var lastDestinationMode: EngineSupervisor.ImportDestinationMode?
    var includesDeferredDate = false

    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan {
        if let error { throw error }
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
                    asShownTime: "12:14",
                    convertedFromUTCTime: "14:14"
                )
            ] + (includesDeferredDate
                ? [
                    .init(
                        source: .filenamePattern,
                        state: .needsConfirmation,
                        itemCount: 1,
                        requiresClock: true,
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
        collectionName: String,
        destinationMode: EngineSupervisor.ImportDestinationMode,
        acceptedDate: String,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview {
        if let error { throw error }
        lastCollectionName = collectionName
        lastDestinationMode = destinationMode
        let scan = try await scanImport(sourcePath: sourcePath)
        let deferred = includesDeferredDate ? 1 : 0
        return .init(
            scan: scan,
            collectionName: collectionName,
            destinationMode: destinationMode,
            destinationPath: "\(catalogPath)/2026/\(acceptedDate) \(collectionName)",
            acceptedDate: acceptedDate,
            dateResolution: dateResolution,
            requiredDecisionCount: dateResolution == nil ? 1 : 0,
            transferCount: dateResolution == nil ? 8 - deferred : 12 - deferred,
            skippedCount: dateResolution == nil ? 4 + deferred : deferred,
            leftOnSourceCount: dateResolution == nil ? 6 + deferred : 2 + deferred,
            operationCount: dateResolution == nil ? 10 - deferred : 14 - deferred,
            requiredSpaceBytes: 41_000_000_000,
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
