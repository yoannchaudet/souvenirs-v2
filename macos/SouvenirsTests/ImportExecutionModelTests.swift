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
            payload: .mediaVerified(
                media: verifiedMedia(),
                completedMedia: 7,
                totalMedia: 12
            )
        ))
        engine.send(.init(
            operationID: "operation-1",
            sequence: 1,
            payload: .progress(
                completedUnits: 40,
                totalUnits: 40,
                message: "Checking copies…"
            )
        ))
        await waitUntil { model.progressMessage == "Checking copies…" }

        XCTAssertEqual(model.phase, .running)
        XCTAssertEqual(model.percentCompleted, 58)
        XCTAssertEqual(model.progressMessage, "Checking copies…")

        let runningRevision = model.presentationRevision
        engine.send(.init(
            operationID: "operation-1",
            sequence: 2,
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
        XCTAssertGreaterThan(model.presentationRevision, runningRevision)
    }

    func testVerifiedMediaUsesMediaCountsAndDeduplicatesRecoveryReplay() async {
        let engine = FakeImportExecutionEngine()
        let model = ImportExecutionCoordinator(engine: engine)
        let media = verifiedMedia()

        model.start(review: review, catalog: catalog)
        await engine.waitForStream()
        let event = EngineSupervisor.OperationEvent(
            operationID: "operation-1",
            sequence: 0,
            payload: .mediaVerified(
                media: media,
                completedMedia: 1,
                totalMedia: 2
            )
        )
        engine.send(event)
        engine.send(.init(
            operationID: "operation-1",
            sequence: 1,
            payload: .mediaVerified(
                media: media,
                completedMedia: 1,
                totalMedia: 2
            )
        ))

        await waitUntil { model.verifiedMedia == [media] }
        XCTAssertEqual(model.completedUnits, 1)
        XCTAssertEqual(model.totalUnits, 2)
    }

    func testStorageProgressCannotReplaceMediaOnlyProgress() async {
        let engine = FakeImportExecutionEngine()
        let model = ImportExecutionCoordinator(engine: engine)
        let media = verifiedMedia()

        model.start(review: review, catalog: catalog)
        await engine.waitForStream()
        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .mediaVerified(
                media: media,
                completedMedia: 1,
                totalMedia: 12
            )
        ))
        engine.send(.init(
            operationID: "operation-1",
            sequence: 1,
            payload: .progress(
                completedUnits: 9,
                totalUnits: 40,
                message: "Updating the manifest…"
            )
        ))

        await waitUntil { model.progressMessage == "Updating the manifest…" }
        XCTAssertEqual(model.completedUnits, 1)
        XCTAssertEqual(model.totalUnits, 12)
        XCTAssertEqual(model.percentCompleted, 8)
    }

    func testConflictingVerifiedMediaFailsClosed() async {
        let engine = FakeImportExecutionEngine()
        let model = ImportExecutionCoordinator(engine: engine)
        let media = verifiedMedia()

        model.start(review: review, catalog: catalog)
        await engine.waitForStream()
        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .mediaVerified(
                media: media,
                completedMedia: 1,
                totalMedia: 2
            )
        ))
        engine.send(.init(
            operationID: "operation-1",
            sequence: 1,
            payload: .mediaVerified(
                media: .init(
                    identity: media.identity,
                    annualPartYear: media.annualPartYear,
                    acceptedDate: media.acceptedDate,
                    filename: "different.jpg",
                    path: media.path
                ),
                completedMedia: 1,
                totalMedia: 2
            )
        ))

        await waitUntil {
            model.phase == .failed("The engine returned inconsistent verified media.")
        }
        XCTAssertEqual(model.verifiedMedia, [media])
    }

    func testInspectionRestoresVerifiedMediaSnapshot() async {
        let engine = FakeImportExecutionEngine()
        let media = verifiedMedia()
        engine.inspection = unfinished(
            sourceAvailable: false,
            verifiedMedia: [media]
        )
        let model = ImportExecutionCoordinator(engine: engine)

        await model.inspect(catalog: catalog)

        XCTAssertEqual(model.phase, .recoveryPrompt(sourceAvailable: false))
        XCTAssertEqual(model.verifiedMedia, [media])
        XCTAssertEqual(model.completedUnits, 7)
        XCTAssertEqual(model.totalUnits, 12)
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
            payload: .mediaVerified(
                media: verifiedMedia(),
                completedMedia: 4,
                totalMedia: 12
            )
        ))
        await waitUntil { appCoordinator?.completedUnits == 4 }

        XCTAssertNil(presentation)
        XCTAssertEqual(appCoordinator?.phase, .running)
        XCTAssertEqual(appCoordinator?.catalog, catalog)
    }

    func testSourceFailureRoutesToReconnectState() async {
        let engine = FakeImportExecutionEngine()
        engine.inspection = unfinished(sourceAvailable: false)
        let model = ImportExecutionCoordinator(engine: engine)

        model.start(review: review, catalog: catalog)
        await engine.waitForStream()
        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .progress(
                completedUnits: 3,
                totalUnits: 12,
                message: "Copying…"
            )
        ))
        engine.send(.init(
            operationID: "operation-1",
            sequence: 1,
            payload: .failed(
                code: "source_unavailable",
                message: "The source was disconnected."
            )
        ))
        engine.finish()

        await waitUntil { model.phase == .recoveryPrompt(sourceAvailable: false) }
        XCTAssertEqual(model.operationID, "operation-1")
        XCTAssertEqual(model.completedUnits, 7)
        XCTAssertEqual(model.totalUnits, 12)
        XCTAssertEqual(model.remainingUnits, 5)
        XCTAssertEqual(model.interruption, .sourceUnavailable)
        XCTAssertEqual(model.recoveryActivityTime?.kind, .interruptedAt)
    }

    func testUnfinishedOperationRoutesBySourceAvailability() async {
        let engine = FakeImportExecutionEngine()
        engine.inspection = unfinished(sourceAvailable: false)
        let model = ImportExecutionCoordinator(engine: engine)

        await model.inspect(catalog: catalog)

        XCTAssertEqual(model.phase, .recoveryPrompt(sourceAvailable: false))
        XCTAssertEqual(model.completedUnits, 7)
        XCTAssertEqual(model.totalUnits, 12)
        XCTAssertEqual(model.remainingUnits, 5)
        XCTAssertEqual(model.interruption, .sourceUnavailable)
        XCTAssertEqual(model.recoveryActivityTime?.kind, .interruptedAt)
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

        let recoveringRevision = model.presentationRevision
        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .cancelled(copiedCount: 7)
        ))
        engine.finish()
        await waitUntil { model.phase == .cancelled(copiedCount: 7) }
        XCTAssertGreaterThan(model.presentationRevision, recoveringRevision)
    }

    func testUnavailableRetryReinspectsAndRecoversSameOperation() async {
        let engine = FakeImportExecutionEngine()
        engine.inspection = unfinished(sourceAvailable: false)
        let model = ImportExecutionCoordinator(engine: engine)
        await model.inspect(catalog: catalog)

        model.deferRecovery()
        XCTAssertEqual(model.phase, .recoveryDeferred)

        model.retryRecovery()
        await waitUntil { model.phase == .recoveryPrompt(sourceAvailable: false) }
        XCTAssertEqual(engine.recoveryStartCount, 0)

        engine.inspection = unfinished(sourceAvailable: true)
        model.retryRecovery()
        await engine.waitForStream()
        XCTAssertEqual(model.phase, .recovering)
        XCTAssertEqual(engine.recoveryStartCount, 1)
        XCTAssertEqual(model.operationID, "operation-1")

        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .completed(
                result: Data(
                    #"{"copied_count":12,"skipped_count":0,"left_on_source_count":0}"#.utf8
                )
            )
        ))
        engine.finish()
        await waitUntil {
            model.phase == .completed(
                ImportOutcome(copiedCount: 12, skippedCount: 0, leftOnSourceCount: 0)
            )
        }
    }

    func testUnavailableRetryKeepsPromptVisibleWhileReinspecting() async {
        let engine = FakeImportExecutionEngine()
        engine.inspection = unfinished(sourceAvailable: false)
        let model = ImportExecutionCoordinator(engine: engine)
        await model.inspect(catalog: catalog)
        engine.blocksInspection = true

        model.retryRecovery()
        await engine.waitForInspection()

        XCTAssertEqual(model.phase, .recoveryPrompt(sourceAvailable: false))
        XCTAssertTrue(model.isRetryingRecovery)

        engine.resumeInspection()
        await waitUntil { !model.isRetryingRecovery }
        XCTAssertEqual(model.phase, .recoveryPrompt(sourceAvailable: false))
    }

    func testDeferredRecoveryBlocksStartingAnotherImport() async {
        let engine = FakeImportExecutionEngine()
        engine.inspection = unfinished(sourceAvailable: false)
        let model = ImportExecutionCoordinator(engine: engine)
        await model.inspect(catalog: catalog)
        model.deferRecovery()

        model.start(review: review, catalog: catalog)

        XCTAssertEqual(model.phase, .recoveryDeferred)
        XCTAssertEqual(engine.importStartCount, 0)
        XCTAssertEqual(model.operationID, "operation-1")
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

    func testCatalogConflictFailureUsesArchiveTerminology() async {
        let engine = FakeImportExecutionEngine()
        let model = ImportExecutionCoordinator(engine: engine)
        model.start(review: review, catalog: catalog)
        await engine.waitForStream()

        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .failed(
                code: "operation_conflict",
                message: "The catalog changed after review. Review the import again."
            )
        ))
        engine.finish()

        await waitUntil {
            model.phase == .failed(
                "The archive changed after review. Review the import again."
            )
        }
    }

    func testCatalogStorageFailureUsesArchiveTerminology() async {
        let engine = FakeImportExecutionEngine()
        let model = ImportExecutionCoordinator(engine: engine)
        model.start(review: review, catalog: catalog)
        await engine.waitForStream()

        engine.send(.init(
            operationID: "operation-1",
            sequence: 0,
            payload: .failed(
                code: "operation_failed",
                message: "failed to validate catalog lock: lock is busy"
            )
        ))
        engine.finish()

        await waitUntil {
            model.phase == .failed(
                "The import couldn’t update this archive. Check its folder or volume, then try again."
            )
        }
    }

    private var catalog: RecentCatalog {
        RecentCatalog(
            path: "/Volumes/photos/Family Catalog",
            name: "Family Archive",
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
                        itemCount: 4,
                        requiresClock: false,
                        supportsUTCConversion: true,
                        asShownTime: "12:14",
                        convertedFromUTCTime: "14:14"
                    )
                ]
            ),
            collectionName: "Dolomites",
            destination: .create(collectionName: "Dolomites"),
            destinationPath: "/Volumes/photos/Family Catalog/2026/2026-10-10 Dolomites",
            acceptedDate: "2026-10-10",
            dateResolution: .asShown,
            requiredDecisionCount: 0,
            transferCount: 12,
            skippedCount: 3,
            leftOnSourceCount: 2,
            operationCount: 14,
            preflight: .fixture,
            canStart: true,
            reviewedPlan: "reviewed-plan"
        )
    }

    private func unfinished(
        sourceAvailable: Bool,
        verifiedMedia: [EngineSupervisor.OperationMedia] = []
    ) -> EngineSupervisor.UnfinishedOperation {
        .init(
            operationID: "operation-1",
            collectionName: "Dolomites",
            sourceName: "EOS_DIGITAL",
            copiedCount: 7,
            totalCount: 12,
            remainingCount: 5,
            verifiedMedia: verifiedMedia,
            sourceAvailable: sourceAvailable,
            interruption: sourceAvailable
                ? .incompleteWithoutTerminalRecord
                : .sourceUnavailable,
            activityTime: .init(
                kind: sourceAvailable ? .lastRecordedActivity : .interruptedAt,
                unixSeconds: 1_760_151_600,
                nanosecond: 0
            )
        )
    }

    private func verifiedMedia() -> EngineSupervisor.OperationMedia {
        .init(
            identity: .dated(
                year: "2026",
                directory: "2026-10-10 +2d Dolomites"
            ),
            annualPartYear: nil,
            acceptedDate: "2026-10-10",
            filename: "20261010-101101-01.jpg",
            path: "/Volumes/photos/Family Catalog/2026/2026-10-10 +2d Dolomites/20261010-101101-01.jpg"
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
    var importStartCount = 0
    var recoveryStartCount = 0
    var cancelledOperationIDs: [String] = []
    private var inspectionContinuation: CheckedContinuation<Void, Never>?
    private var continuation:
        AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error>.Continuation?

    func startImportExecution(reviewedPlan: String) async throws -> String {
        importStartCount += 1
        return "operation-1"
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
        let activeContinuation = continuation
        continuation = nil
        activeContinuation?.finish()
    }
}
