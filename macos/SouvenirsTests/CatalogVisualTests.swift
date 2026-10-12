import AppKit
import SwiftUI
import XCTest
@testable import Souvenirs

@MainActor
final class CatalogVisualTests: XCTestCase {
    func testCaptureCatalogFlowScreens() async throws {
        let engine = VisualCatalogEngine()
        let model = CatalogWelcomeModel(
            engine: engine,
            sourceDiscovery: DebugSourceDiscovery(scenario: .connected),
            defaults: UserDefaults(suiteName: "CatalogVisualTests")!,
            recents: [
                RecentCatalog(
                    path: "/Volumes/photos/Family Catalog",
                    name: "Family Archive",
                    volumeName: "photos-nas.local"
                )
            ]
        )
        let imports = ImportExecutionCoordinator(engine: engine)

        try capture(
            ContentView(
                engineStatus: .connecting,
                model: model,
                imports: imports
            ),
            size: NSSize(width: 586, height: 386),
            name: "catalog-welcome-connecting",
            assertsHeightFits: true
        )

        try capture(
            ContentView(
                engineStatus: .failed("The test engine could not start."),
                model: model,
                imports: imports,
                onRetryEngine: {}
            ),
            size: NSSize(width: 586, height: 386),
            name: "catalog-welcome-engine-failed",
            assertsHeightFits: true
        )

        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: imports
            ),
            size: NSSize(width: 920, height: 620),
            name: "catalog-welcome",
            assertsHeightFits: true
        )

        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: imports
            ),
            size: NSSize(width: 586, height: 386),
            name: "catalog-welcome-minimum",
            assertsHeightFits: true
        )

        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: imports
            ),
            size: NSSize(width: 1_200, height: 800),
            name: "catalog-welcome-enlarged",
            assertsHeightFits: true
        )

        model.beginCreate()
        try capture(
            CatalogCreationSheet(
                model: model,
                onChooseLocation: {},
                onCancel: {},
                onCreate: {}
            ),
            size: NSSize(width: 540, height: 500),
            name: "catalog-create-empty",
            assertsHeightFits: true
        )

        await model.chooseCreateLocation("/Volumes/photos/Family Catalog")
        try capture(
            CatalogCreationSheet(
                model: model,
                onChooseLocation: {},
                onCancel: {},
                onCreate: {}
            ),
            size: NSSize(width: 540, height: 500),
            name: "catalog-create",
            assertsHeightFits: true
        )

        engine.validationError = EngineSupervisor.SupervisorError.protocolError(
            code: "catalog_location_invalid",
            message: "This folder is read-only. Choose a writable folder."
        )
        await model.chooseCreateLocation("/Volumes/READ_ONLY/Family Catalog")
        try capture(
            CatalogCreationSheet(
                model: model,
                onChooseLocation: {},
                onCancel: {},
                onCreate: {}
            ),
            size: NSSize(width: 540, height: 500),
            name: "catalog-create-invalid",
            assertsHeightFits: true
        )

        engine.validationError = nil
        await model.chooseCreateLocation("/Volumes/photos/Family Catalog")
        await model.createCatalog()
        for _ in 0..<20 where model.selectedConnectedSource == nil {
            await Task.yield()
        }
        XCTAssertNotNil(model.selectedConnectedSource)
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: imports
            ),
            size: NSSize(width: 920, height: 620),
            name: "catalog-ready"
        )
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: imports
            ),
            size: NSSize(width: 586, height: 386),
            name: "catalog-ready-minimum"
        )
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: imports
            ),
            size: NSSize(width: 1_200, height: 800),
            name: "catalog-ready-enlarged"
        )

        if case .catalog(let catalog) = model.route {
            engine.browseCollections = VisualCatalogEngine.browseFixture
            await model.browse.refresh()
            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 920, height: 620),
                name: "archive-browse-overview"
            )
            model.browse.selectScope(.year(2025))
            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 920, height: 620),
                name: "archive-browse-year"
            )
            model.browse.selectScope(.earlier(throughYear: 2019))
            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 920, height: 620),
                name: "archive-browse-earlier"
            )
            await model.browse.select(
                .init(identity: .ongoing(key: "kids"), annualPartYear: 2025)
            )
            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 586, height: 386),
                name: "archive-browse-detail-problem"
            )

            engine.scanSuspended = true
            model.beginImport(from: "/Volumes/EOS_DIGITAL", into: catalog)
            for _ in 0..<20 where model.importPlanning?.isLoading != true {
                await Task.yield()
            }
            XCTAssertTrue(model.importPlanning?.isLoading == true)
            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 586, height: 386),
                name: "import-review-loading-small"
            )

            engine.resumeScan()
            for _ in 0..<20 where model.importPlanning?.review == nil {
                await Task.yield()
            }
            XCTAssertNotNil(model.importPlanning?.review)
            XCTAssertFalse(model.importPlanning?.isLoading ?? true)

            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 920, height: 620),
                name: "import-review-decision"
            )
            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 586, height: 386),
                name: "import-review-decision-small"
            )

            model.importPlanning?.showsDetails = true
            XCTAssertTrue(model.importPlanning?.showsDetails == true)
            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 920, height: 620),
                name: "import-review-details"
            )

            engine.reviewError = EngineSupervisor.SupervisorError.protocolError(
                code: "review_failed",
                message: "The source could not be reviewed."
            )
            await model.importPlanning?.refreshReview()
            XCTAssertEqual(
                model.importPlanning?.errorMessage,
                "The source could not be reviewed."
            )
            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 920, height: 620),
                name: "import-review-error"
            )
        }
    }

    func testCaptureConnectedSourceStates() async throws {
        let catalog = RecentCatalog(
            path: "/Volumes/photos/Family Catalog",
            name: "Family Archive",
            volumeName: "photos-nas.local"
        )

        for (scenario, name) in [
            (DebugSourceDiscovery.Scenario.multiple, "connected-sources-multiple"),
            (.errorThenConnected, "connected-sources-error")
        ] {
            let engine = VisualCatalogEngine()
            let model = CatalogWelcomeModel(
                engine: engine,
                sourceDiscovery: DebugSourceDiscovery(scenario: scenario),
                recents: [catalog]
            )
            let imports = ImportExecutionCoordinator(engine: engine)
            await model.openCatalog(at: catalog.path)
            for _ in 0..<20
            where model.isDiscoveringSources && model.sourceDiscoveryError == nil {
                await Task.yield()
            }

            try capture(
                ContentView(
                    engineStatus: .ready(engineVersion: "0.1.0"),
                    model: model,
                    imports: imports
                ),
                size: NSSize(width: 920, height: 620),
                name: name
            )
        }
    }

    func testCaptureImportExecutionAndRecoveryScreens() async throws {
        let catalog = RecentCatalog(
            path: "/Volumes/photos/Family Catalog",
            name: "Family Archive",
            volumeName: "photos-nas.local"
        )
        let modelEngine = VisualCatalogEngine()
        let model = CatalogWelcomeModel(
            engine: modelEngine,
            sourceDiscovery: DebugSourceDiscovery(scenario: .connected),
            recents: [catalog]
        )
        await model.openCatalog(at: catalog.path)

        let runningEngine = VisualCatalogEngine()
        let running = ImportExecutionCoordinator(engine: runningEngine)
        running.start(review: try await runningEngine.review(), catalog: catalog)
        await runningEngine.waitForStream()
        runningEngine.send(.init(
            operationID: "visual-operation",
            sequence: 0,
            payload: .progress(
                completedUnits: 7,
                totalUnits: 12,
                message: "Reading back and checking each copy…"
            )
        ))
        for _ in 0..<20 {
            if running.completedUnits == 7 { break }
            await Task.yield()
        }
        XCTAssertEqual(running.completedUnits, 7)
        XCTAssertEqual(running.phase, .running)
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: running
            ),
            size: NSSize(width: 920, height: 620),
            name: "import-execution"
        )
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: running
            ),
            size: NSSize(width: 586, height: 386),
            name: "import-execution-small"
        )

        running.stop()
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: running
            ),
            size: NSSize(width: 920, height: 620),
            name: "import-stopping"
        )
        runningEngine.send(.init(
            operationID: "visual-operation",
            sequence: 1,
            payload: .cancelled(copiedCount: 7)
        ))
        for _ in 0..<20 {
            if case .cancelled = running.phase { break }
            await Task.yield()
        }
        XCTAssertEqual(running.phase, .cancelled(copiedCount: 7))
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: running
            ),
            size: NSSize(width: 920, height: 620),
            name: "import-cancelled"
        )
        runningEngine.finish()

        let completedEngine = VisualCatalogEngine()
        let completed = ImportExecutionCoordinator(engine: completedEngine)
        completed.start(review: try await completedEngine.review(), catalog: catalog)
        await completedEngine.waitForStream()
        completedEngine.send(.init(
            operationID: "visual-operation",
            sequence: 0,
            payload: .progress(
                completedUnits: 12,
                totalUnits: 12,
                message: "Reading back and checking each copy…"
            )
        ))
        completedEngine.send(.init(
            operationID: "visual-operation",
            sequence: 1,
            payload: .completed(
                result: Data(
                    #"{"copied_count":12,"skipped_count":3,"left_on_source_count":2}"#.utf8
                )
            )
        ))
        completedEngine.finish()
        for _ in 0..<10 {
            if case .completed = completed.phase { break }
            await Task.yield()
        }
        XCTAssertEqual(
            completed.phase,
            .completed(
                ImportOutcome(
                    copiedCount: 12,
                    skippedCount: 3,
                    leftOnSourceCount: 2
                )
            )
        )
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: completed
            ),
            size: NSSize(width: 920, height: 620),
            name: "import-completed"
        )

        let failedEngine = VisualCatalogEngine()
        let failed = ImportExecutionCoordinator(engine: failedEngine)
        failed.start(review: try await failedEngine.review(), catalog: catalog)
        await failedEngine.waitForStream()
        failedEngine.send(.init(
            operationID: "visual-operation",
            sequence: 0,
            payload: .failed(
                code: "copy_failed",
                message: "The destination stopped responding."
            )
        ))
        for _ in 0..<20 {
            if case .failed = failed.phase { break }
            await Task.yield()
        }
        XCTAssertEqual(failed.phase, .failed("The destination stopped responding."))
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: failed
            ),
            size: NSSize(width: 920, height: 620),
            name: "import-failed"
        )
        failedEngine.finish()

        let recoveryPresentEngine = VisualCatalogEngine()
        recoveryPresentEngine.inspection = .init(
            operationID: "interrupted-operation",
            collectionName: "Dolomites",
            sourceName: "EOS_DIGITAL",
            copiedCount: 7,
            totalCount: 12,
            remainingCount: 5,
            sourceAvailable: true,
            interruption: .incompleteWithoutTerminalRecord,
            activityTime: .init(
                kind: .lastRecordedActivity,
                unixSeconds: 1_760_151_600,
                nanosecond: 0
            )
        )
        let recoveryPresent = ImportExecutionCoordinator(engine: recoveryPresentEngine)
        await recoveryPresent.inspect(catalog: catalog)
        XCTAssertEqual(recoveryPresent.phase, .recoveryPrompt(sourceAvailable: true))
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: recoveryPresent
            ),
            size: NSSize(width: 920, height: 620),
            name: "import-recovery-source-present"
        )

        let recoveryMissingEngine = VisualCatalogEngine()
        recoveryMissingEngine.inspection = .init(
            operationID: "interrupted-operation",
            collectionName: "Dolomites",
            sourceName: "EOS_DIGITAL",
            copiedCount: 7,
            totalCount: 12,
            remainingCount: 5,
            sourceAvailable: false,
            interruption: .sourceUnavailable,
            activityTime: .init(
                kind: .interruptedAt,
                unixSeconds: 1_760_151_600,
                nanosecond: 0
            )
        )
        let recoveryMissing = ImportExecutionCoordinator(engine: recoveryMissingEngine)
        await recoveryMissing.inspect(catalog: catalog)
        XCTAssertEqual(recoveryMissing.phase, .recoveryPrompt(sourceAvailable: false))
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: recoveryMissing
            ),
            size: NSSize(width: 920, height: 620),
            name: "import-recovery-source-missing"
        )
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: recoveryMissing
            ),
            size: NSSize(width: 586, height: 386),
            name: "import-recovery-source-missing-small"
        )

        recoveryMissing.deferRecovery()
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: recoveryMissing
            ),
            size: NSSize(width: 920, height: 620),
            name: "import-recovery-deferred"
        )
        try capture(
            ContentView(
                engineStatus: .ready(engineVersion: "0.1.0"),
                model: model,
                imports: recoveryMissing
            ),
            size: NSSize(width: 586, height: 386),
            name: "import-recovery-deferred-small"
        )
    }

    private func capture<ViewType: View>(
        _ view: ViewType,
        size: NSSize,
        name: String,
        assertsHeightFits: Bool = false
    ) throws {
        let hostingController = NSHostingController(rootView: view)
        let hostingView = hostingController.view
        let fittingSize = hostingController.sizeThatFits(in: size)
        XCTAssertLessThanOrEqual(
            fittingSize.width,
            size.width + 1,
            "\(name) requires \(fittingSize.width)pt but only \(size.width)pt is available"
        )
        if assertsHeightFits {
            XCTAssertLessThanOrEqual(
                fittingSize.height,
                size.height + 1,
                "\(name) requires \(fittingSize.height)pt but only \(size.height)pt is available"
            )
        }
        hostingView.frame = NSRect(origin: .zero, size: size)
        hostingView.layoutSubtreeIfNeeded()
        guard let representation = hostingView.bitmapImageRepForCachingDisplay(
            in: hostingView.bounds
        ) else {
            return XCTFail("Could not create a bitmap for \(name)")
        }
        hostingView.cacheDisplay(in: hostingView.bounds, to: representation)
        guard let data = representation.representation(using: .png, properties: [:]) else {
            return XCTFail("Could not encode \(name)")
        }

        let directory = URL(fileURLWithPath: "/tmp/SouvenirsScreenshots")
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        let url = directory.appendingPathComponent("\(name).png")
        try data.write(to: url)

        let attachment = XCTAttachment(data: data, uniformTypeIdentifier: "public.png")
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}

@MainActor
private final class VisualCatalogEngine: CatalogEngineClient, ImportExecutionEngine {
    var inspection: EngineSupervisor.UnfinishedOperation?
    var validationError: Error?
    var reviewError: Error?
    var scanSuspended = false
    var browseCollections: [EngineSupervisor.CatalogBrowseCollection] = []
    private var continuation:
        AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error>.Continuation?
    private var scanContinuation: CheckedContinuation<Void, Never>?
    func validateCatalog(path: String, intent: String) async throws
        -> EngineSupervisor.CatalogLocation {
        if let validationError {
            throw validationError
        }
        return location(path)
    }

    func createCatalog(
        path: String,
        displayName: String
    ) async throws -> EngineSupervisor.CatalogLocation {
        location(path)
    }

    func openCatalog(path: String) async throws -> EngineSupervisor.CatalogLocation {
        location(path)
    }

    func catalogCollections(
        path: String
    ) async throws -> [EngineSupervisor.CatalogCollection] {
        []
    }

    func catalogBrowse(
        path: String,
        selection: EngineSupervisor.CatalogCollectionIdentity?
    ) async throws -> EngineSupervisor.CatalogBrowseSnapshot {
        guard let selection else {
            return .init(
                navigation: Self.browseNavigation,
                collections: browseCollections.map(\.overviewOnly)
            )
        }
        return .init(
            navigation: [],
            collections: browseCollections.filter { $0.identity == selection }
        )
    }

    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan {
        if scanSuspended {
            await withCheckedContinuation { continuation in
                scanContinuation = continuation
            }
        }
        return .init(
            sourcePath: sourcePath,
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
        )
    }

    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        destination: EngineSupervisor.ImportDestination,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview {
        if let reviewError {
            throw reviewError
        }
        let scan = try await scanImport(sourcePath: sourcePath)
        let collectionName: String
        switch destination {
        case .create(let name):
            collectionName = name
        case .existing:
            collectionName = "Existing"
        }
        return .init(
            scan: scan,
            collectionName: collectionName,
            destination: destination,
            destinationPath: "\(catalogPath)/2026/2026-10-10 \(collectionName)",
            acceptedDate: "2026-10-10",
            dateResolution: dateResolution,
            requiredDecisionCount: dateResolution == nil ? 1 : 0,
            transferCount: dateResolution == nil ? 8 : 12,
            skippedCount: dateResolution == nil ? 4 : 0,
            leftOnSourceCount: dateResolution == nil ? 6 : 2,
            operationCount: dateResolution == nil ? 10 : 14,
            preflight: .fixture,
            canStart: true,
            reviewedPlan: "reviewed-plan"
        )
    }

    private func location(_ path: String) -> EngineSupervisor.CatalogLocation {
        .init(
            path: path,
            volumeName: "photos-nas.local",
            displayName: "Family Archive",
            detail: "Catalog format 2"
        )
    }

    static var browseFixture: [EngineSupervisor.CatalogBrowseCollection] {
        let datedMedia = EngineSupervisor.CatalogBrowseMedia(
            filename: "2019-07-15 10-11-12 001.jpg",
            path: "/unavailable/brittany.jpg"
        )
        let kidsMedia = EngineSupervisor.CatalogBrowseMedia(
            filename: "2024-06-08 09-10-11 001.jpg",
            path: "/unavailable/kids.jpg"
        )
        return [
            .dated(
                identity: .dated(
                    year: "2019",
                    directory: "2019-07-14 +6d Brittany"
                ),
                label: "Brittany",
                startDate: "2019-07-14",
                endDate: "2019-07-20",
                mediaCount: 1,
                coverMedia: datedMedia,
                dateGroups: [.init(date: "2019-07-15", media: [datedMedia])],
                problem: nil
            ),
            .dated(
                identity: .dated(year: "2022", directory: "2022-01-01 Empty"),
                label: "Empty",
                startDate: "2022-01-01",
                endDate: "2022-01-01",
                mediaCount: 0,
                coverMedia: nil,
                dateGroups: [],
                problem: nil
            ),
            .ongoing(
                identity: .ongoing(key: "kids"),
                label: "Kids",
                mediaCount: 2,
                parts: [
                    .init(
                        year: 2024,
                        yearDirectory: "2024",
                        collectionDirectory: "z Kids",
                        mediaCount: 1,
                        coverMedia: kidsMedia,
                        dateGroups: [.init(date: "2024-06-08", media: [kidsMedia])],
                        problem: nil
                    ),
                    .init(
                        year: 2025,
                        yearDirectory: "2025",
                        collectionDirectory: "z Kids",
                        mediaCount: 1,
                        coverMedia: kidsMedia,
                        dateGroups: [.init(date: "2025-01-01", media: [kidsMedia])],
                        problem: "SHA256SUMS contains a malformed manifest line."
                    )
                ]
            )
        ]
    }

    static var browseNavigation: [EngineSupervisor.CatalogBrowseNavigationScope] {
        [
            .allPhotos,
            .year(
                year: 2025,
                entries: [
                    .init(identity: .ongoing(key: "kids"), annualPartYear: 2025)
                ]
            ),
            .year(
                year: 2024,
                entries: [
                    .init(identity: .ongoing(key: "kids"), annualPartYear: 2024)
                ]
            ),
            .year(
                year: 2022,
                entries: [
                    .init(
                        identity: .dated(year: "2022", directory: "2022-01-01 Empty"),
                        annualPartYear: nil
                    )
                ]
            ),
            .earlier(
                throughYear: 2019,
                years: [
                    .init(
                        year: 2019,
                        entries: [
                            .init(
                                identity: .dated(
                                    year: "2019",
                                    directory: "2019-07-14 +6d Brittany"
                                ),
                                annualPartYear: nil
                            )
                        ]
                    )
                ]
            ),
        ]
    }

    func startImportExecution(reviewedPlan: String) async throws -> String {
        "visual-operation"
    }
    func inspectUnfinishedOperation(
        catalogPath: String
    ) async throws -> EngineSupervisor.UnfinishedOperation? { inspection }
    func recoverOperation(catalogPath: String, operationID: String) async throws -> String {
        "visual-recovery"
    }
    func cancel(
        operationID: String
    ) async throws -> EngineSupervisor.CancellationAccepted {
        .init(operationID: operationID)
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

    func resumeScan() {
        scanSuspended = false
        scanContinuation?.resume()
        scanContinuation = nil
    }

    func send(_ event: EngineSupervisor.OperationEvent) {
        continuation?.yield(event)
    }

    func finish() {
        continuation?.finish()
    }

    func review() async throws -> EngineSupervisor.ImportReview {
        try await reviewImport(
            catalogPath: "/Volumes/photos/Family Catalog",
            sourcePath: "/Volumes/EOS_DIGITAL",
            destination: .create(collectionName: "Dolomites"),
            dateResolution: .asShown
        )
    }
}
