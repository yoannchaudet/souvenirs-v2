import XCTest
@testable import Souvenirs

@MainActor
final class ImportExecutionModelTests: XCTestCase {
    func testProgressAndCompletionUseEngineEvents() async {
        let engine = FakeImportExecutionEngine()
        let model = ImportExecutionCoordinator(engine: engine)

        model.start(review: review, catalog: catalog)
        await engine.waitForStream()
        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .progress(
                completedUnits: 7,
                totalUnits: 12,
                message: "Checking copies…"
            )
        ))
        await waitUntil { model.completedUnits == 7 }

        XCTAssertEqual(model.phase, .running)
        XCTAssertEqual(model.percentCompleted, 58)
        XCTAssertEqual(model.progressMessage, "Checking copies…")

        engine.send(.init(
            operationID: "operation-1",
            sequence: 1,
            payload: .completed(
                result: Data(
                    #"{"copied_count":12,"skipped_count":3,"left_on_source_count":2}"#.utf8
                )
            )
        ))
        engine.finish()
        await waitUntil {
            model.phase == .completed(
                ImportOutcome(copiedCount: 12, skippedCount: 3, leftOnSourceCount: 2)
            )
        }
    }

    func testExecutionSurvivesPresentationLifecycle() async {
        let engine = FakeImportExecutionEngine()
        @MainActor final class AppOwner {
            let coordinator: ImportExecutionCoordinator
            init(engine: ImportExecutionEngine) {
                coordinator = ImportExecutionCoordinator(engine: engine)
            }
        }
        final class Presentation {
            weak var coordinator: ImportExecutionCoordinator?
            init(coordinator: ImportExecutionCoordinator) {
                self.coordinator = coordinator
            }
        }

        let owner = AppOwner(engine: engine)
        weak var appCoordinator = owner.coordinator
        owner.coordinator.start(review: review, catalog: catalog)
        await engine.waitForStream()
        var presentation: Presentation? = Presentation(coordinator: owner.coordinator)
        presentation = nil
        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .progress(
                completedUnits: 4,
                totalUnits: 12,
                message: "Still copying…"
            )
        ))
        await waitUntil { appCoordinator?.completedUnits == 4 }

        XCTAssertNil(presentation)
        XCTAssertEqual(appCoordinator?.phase, .running)
        XCTAssertEqual(appCoordinator?.catalog, catalog)
    }

    func testSourceFailureRoutesToReconnectState() async {
        let engine = FakeImportExecutionEngine()
        let model = ImportExecutionCoordinator(engine: engine)

        model.start(review: review, catalog: catalog)
        await engine.waitForStream()
        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .failed(
                code: "source_unavailable",
                message: "The source was disconnected."
            )
        ))
        engine.finish()

        await waitUntil { model.phase == .sourceUnavailable }
    }

    func testUnfinishedOperationRoutesBySourceAvailability() async {
        let engine = FakeImportExecutionEngine()
        engine.inspection = unfinished(sourceAvailable: false)
        let model = ImportExecutionCoordinator(engine: engine)

        await model.inspect(catalog: catalog)

        XCTAssertEqual(model.phase, .recoveryPrompt(sourceAvailable: false))
        XCTAssertEqual(model.completedUnits, 7)
        XCTAssertEqual(model.totalUnits, 12)
        XCTAssertEqual(model.sourceName, "EOS_DIGITAL")
    }

    func testRecoveryAndCancellationUseEngineOperations() async {
        let engine = FakeImportExecutionEngine()
        engine.inspection = unfinished(sourceAvailable: true)
        let model = ImportExecutionCoordinator(engine: engine)
        await model.inspect(catalog: catalog)

        model.finishRecovery()
        await engine.waitForStream()
        XCTAssertEqual(model.phase, .recovering)
        XCTAssertEqual(engine.recoveryStartCount, 1)

        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .cancelled
        ))
        engine.finish()
        await waitUntil { model.phase == .cancelled(copiedCount: 7) }
    }

    func testStoppingRecoveryWaitsForEngineAcknowledgement() async {
        let engine = FakeImportExecutionEngine()
        engine.inspection = unfinished(sourceAvailable: true)
        let model = ImportExecutionCoordinator(engine: engine)
        await model.inspect(catalog: catalog)

        model.stopRecoveryHere()
        XCTAssertEqual(model.phase, .stopping)
        await waitUntil { model.phase == .cancelled(copiedCount: 7) }
        XCTAssertEqual(engine.cancelledOperationIDs, ["operation-1"])
    }

    func testInspectionFailureIsVisibleAndDismissible() async {
        let engine = FakeImportExecutionEngine()
        engine.inspectionError = TestError.inspection
        let model = ImportExecutionCoordinator(engine: engine)

        await model.inspect(catalog: catalog)

        XCTAssertEqual(model.phase, .failed("Inspection failed."))
        XCTAssertEqual(model.catalog, catalog)
        model.dismissOutcome()
        XCTAssertEqual(model.phase, .idle)
    }

    func testLateInspectionFailureDoesNotClobberRunningImport() async {
        let engine = FakeImportExecutionEngine()
        engine.blocksInspection = true
        engine.inspectionError = TestError.inspection
        let model = ImportExecutionCoordinator(engine: engine)
        let inspection = Task { await model.inspect(catalog: catalog) }
        await engine.waitForInspection()

        model.start(review: review, catalog: catalog)
        engine.resumeInspection()
        await inspection.value

        XCTAssertEqual(model.phase, .running)
    }

    func testMalformedCompletionFailsClosed() async {
        let engine = FakeImportExecutionEngine()
        let model = ImportExecutionCoordinator(engine: engine)
        model.start(review: review, catalog: catalog)
        await engine.waitForStream()

        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .completed(result: Data(#"{"copied_count":12}"#.utf8))
        ))
        await waitUntil {
            model.phase == .failed("The engine returned an invalid completion result.")
        }
    }

    private var catalog: RecentCatalog {
        RecentCatalog(
            path: "/Volumes/photos/Family Catalog",
            name: "Family Catalog",
            volumeName: "photos-nas.local"
        )
    }

    private var review: EngineSupervisor.ImportReview {
        .init(
            scan: .init(
                sourcePath: "/Volumes/EOS_DIGITAL",
                sourceName: "EOS_DIGITAL",
                fileCount: 17,
                mediaCount: 12,
                photoCount: 8,
                videoCount: 4,
                unsupportedCount: 2,
                uncertainTimeCount: 4,
                totalBytes: 41_000_000_000,
                suggestedCollectionName: "Dolomites",
                suggestedAcceptedDate: "2026-10-10",
                suggestedRangeEnd: "2026-10-12",
                pendingItemCount: 4,
                unresolvedItemCount: 0,
                datetimeSources: [
                    .init(
                        source: .containerCreationTime,
                        state: .needsConfirmation,
                        itemCount: 4
                    )
                ]
            ),
            collectionName: "Dolomites",
            destinationMode: .create,
            destinationPath: "/Volumes/photos/Family Catalog/2026/2026-10-10 Dolomites",
            acceptedDate: "2026-10-10",
            dateResolution: .asShown,
            requiredDecisionCount: 0,
            transferCount: 12,
            skippedCount: 3,
            leftOnSourceCount: 2,
            operationCount: 14,
            requiredSpaceBytes: 41_000_000_000,
            canStart: true,
            reviewedPlan: "reviewed-plan"
        )
    }

    private func unfinished(
        sourceAvailable: Bool
    ) -> EngineSupervisor.UnfinishedOperation {
        .init(
            operationID: "operation-1",
            collectionName: "Dolomites",
            sourceName: "EOS_DIGITAL",
            copiedCount: 7,
            totalCount: 12,
            sourceAvailable: sourceAvailable
        )
    }

    private func waitUntil(
        timeout: Duration = .seconds(2),
        _ predicate: @escaping @MainActor () -> Bool
    ) async {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while !predicate(), clock.now < deadline {
            await Task.yield()
        }
        XCTAssertTrue(predicate())
    }
}

private enum TestError: LocalizedError {
    case inspection

    var errorDescription: String? { "Inspection failed." }
}

@MainActor
private final class FakeImportExecutionEngine: ImportExecutionEngine {
    var inspection: EngineSupervisor.UnfinishedOperation?
    var inspectionError: Error?
    var blocksInspection = false
    var recoveryStartCount = 0
    var cancelledOperationIDs: [String] = []
    private var inspectionContinuation: CheckedContinuation<Void, Never>?
    private var continuation:
        AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error>.Continuation?

    func startImportExecution(reviewedPlan: String) async throws -> String {
        "operation-1"
    }

    func inspectUnfinishedOperation(
        catalogPath: String
    ) async throws -> EngineSupervisor.UnfinishedOperation? {
        if blocksInspection {
            await withCheckedContinuation { inspectionContinuation = $0 }
        }
        if let inspectionError { throw inspectionError }
        return inspection
    }

    func recoverOperation(catalogPath: String, operationID: String) async throws -> String {
        recoveryStartCount += 1
        return "operation-1"
    }

    func cancel(
        operationID: String
    ) async throws -> EngineSupervisor.CancellationAccepted {
        cancelledOperationIDs.append(operationID)
        return .init(operationID: operationID)
    }

    func events(
        for operationID: String
    ) async throws -> AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error> {
        AsyncThrowingStream { continuation in
            self.continuation = continuation
        }
    }

    func waitForStream() async {
        while continuation == nil {
            await Task.yield()
        }
    }

    func waitForInspection() async {
        while inspectionContinuation == nil {
            await Task.yield()
        }
    }

    func resumeInspection() {
        inspectionContinuation?.resume()
        inspectionContinuation = nil
    }

    func send(_ event: EngineSupervisor.OperationEvent) {
        continuation?.yield(event)
    }

    func finish() {
        continuation?.finish()
    }
}
