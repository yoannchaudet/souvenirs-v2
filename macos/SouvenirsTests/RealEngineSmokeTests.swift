import CryptoKit
import Darwin
import Foundation
import XCTest
@testable import Souvenirs

final class RealEngineSmokeTests: XCTestCase {
    private var temporaryDirectories: [URL] = []

    override func tearDownWithError() throws {
        for directory in temporaryDirectories {
            try? FileManager.default.removeItem(at: directory)
        }
        temporaryDirectories.removeAll()
        try super.tearDownWithError()
    }

    func testRealEngineCompletesImportAndWritesVerifiedManifest() async throws {
        let fixture = try makeFixture(fileCount: 2, bytesPerFile: 32)
        let supervisor = EngineSupervisor()
        _ = try await supervisor.start()
        addTeardownBlock {
            try? await supervisor.stop()
        }
        let handshake = try await supervisor.handshake()
        XCTAssertTrue(handshake.capabilities.contains("operation.start"))

        let location = try await supervisor.createCatalog(
            path: fixture.catalog.path,
            displayName: "Smoke Catalog"
        )
        XCTAssertEqual(location.displayName, "Smoke Catalog")
        let review = try await plannedImport(
            supervisor: supervisor,
            fixture: fixture,
            collectionName: "Smoke Import"
        )
        XCTAssertTrue(review.canStart)

        let operationID = try await supervisor.requestOperation(
            method: "operation.start",
            parameters: StartImportParameters(
                reviewedPlan: try XCTUnwrap(review.reviewedPlan)
            )
        )
        let events = try await collectEvents(
            try await supervisor.events(for: operationID)
        )
        XCTAssertEqual(events.map(\.sequence), Array(0..<UInt64(events.count)))
        XCTAssertTrue(events.contains { event in
            if case .progress(completedUnits: 2, totalUnits: 2, message: _) = event.payload {
                return true
            }
            return false
        })
        guard case .completed = events.last?.payload else {
            return XCTFail("Expected a completed terminal event")
        }

        try verifyCatalog(
            fixture.catalog,
            collectionName: "Smoke Import",
            expectedFiles: 2
        )
        let completedInspection = try await supervisor.inspectUnfinishedOperation(
            catalogPath: fixture.catalog.path
        )
        XCTAssertNil(completedInspection)
        try await supervisor.stop()
    }

    func testRealEngineCancellationInterruptsRemainingCopies() async throws {
        let fixture = try makeFixture(fileCount: 3, bytesPerFile: 32)
        let supervisor = EngineSupervisor(
            arguments: ["--test-progress-delay-ms", "2000"]
        )
        _ = try await supervisor.start()
        addTeardownBlock {
            try? await supervisor.stop()
        }
        _ = try await supervisor.handshake()
        _ = try await supervisor.createCatalog(
            path: fixture.catalog.path,
            displayName: "Cancellation Catalog"
        )
        let review = try await plannedImport(
            supervisor: supervisor,
            fixture: fixture,
            collectionName: "Cancellation Import"
        )
        let operationID = try await supervisor.requestOperation(
            method: "operation.start",
            parameters: StartImportParameters(
                reviewedPlan: try XCTUnwrap(review.reviewedPlan)
            )
        )
        let stream = try await supervisor.events(for: operationID)
        var events: [EngineSupervisor.OperationEvent] = []
        var cancellationAccepted = false
        for try await event in stream {
            events.append(event)
            if !cancellationAccepted,
               case .progress(
                   completedUnits: 1,
                   totalUnits: _,
                   message: _
               ) = event.payload {
                let activeInspection =
                    try await supervisor.inspectUnfinishedOperation(
                        catalogPath: fixture.catalog.path
                    )
                XCTAssertNil(activeInspection)
                let accepted = try await supervisor.cancel(operationID: operationID)
                XCTAssertEqual(accepted.operationID, operationID)
                cancellationAccepted = true
            }
        }

        XCTAssertTrue(cancellationAccepted)
        XCTAssertEqual(events.map(\.sequence), Array(0..<UInt64(events.count)))
        XCTAssertEqual(events.filter(\.isTerminal).count, 1)
        guard case .cancelled(copiedCount: 1) = events.last?.payload else {
            return XCTFail("Expected cancellation after the first durable copy")
        }
        try verifyCatalog(
            fixture.catalog,
            collectionName: "Cancellation Import",
            expectedFiles: 1
        )
        try verifyCancelledJournal(fixture.catalog, operationID: operationID)
        let unfinished = try await supervisor.inspectUnfinishedOperation(
            catalogPath: fixture.catalog.path
        )
        XCTAssertNil(unfinished)
        try await supervisor.stop()
    }

    func testRealEngineRecoversAfterSourceDisconnectAndExplicitRetry() async throws {
        let fixture = try makeFixture(fileCount: 3, bytesPerFile: 32)
        let detachedSource = fixture.source
            .deletingLastPathComponent()
            .appendingPathComponent("detached-source")
        let supervisor = EngineSupervisor(
            arguments: ["--test-progress-delay-ms", "2000"]
        )
        _ = try await supervisor.start()
        addTeardownBlock {
            try? await supervisor.stop()
        }
        _ = try await supervisor.handshake()
        _ = try await supervisor.createCatalog(
            path: fixture.catalog.path,
            displayName: "Source Recovery Catalog"
        )
        let review = try await plannedImport(
            supervisor: supervisor,
            fixture: fixture,
            collectionName: "Source Recovery Import"
        )
        let operationID = try await supervisor.requestOperation(
            method: "operation.start",
            parameters: StartImportParameters(
                reviewedPlan: try XCTUnwrap(review.reviewedPlan)
            )
        )
        let stream = try await supervisor.events(for: operationID)
        var events: [EngineSupervisor.OperationEvent] = []
        var sourceDetached = false
        for try await event in stream {
            events.append(event)
            if !sourceDetached,
               case .progress(completedUnits: 1, totalUnits: _, message: _) = event.payload {
                try FileManager.default.moveItem(
                    at: fixture.source,
                    to: detachedSource
                )
                sourceDetached = true
            }
        }

        XCTAssertTrue(sourceDetached)
        XCTAssertEqual(events.map(\.sequence), Array(0..<UInt64(events.count)))
        guard case .failed(let code, _) = events.last?.payload else {
            return XCTFail("Expected source-unavailable terminal event")
        }
        XCTAssertEqual(code, "source_unavailable")

        let unfinished = try await supervisor.inspectUnfinishedOperation(
            catalogPath: fixture.catalog.path
        )
        XCTAssertEqual(unfinished?.operationID, operationID)
        XCTAssertEqual(unfinished?.copiedCount, 1)
        XCTAssertEqual(unfinished?.totalCount, 3)
        XCTAssertEqual(unfinished?.sourceAvailable, false)

        try FileManager.default.moveItem(at: detachedSource, to: fixture.source)
        let reconnected = try await supervisor.inspectUnfinishedOperation(
            catalogPath: fixture.catalog.path
        )
        XCTAssertEqual(reconnected?.operationID, operationID)
        XCTAssertEqual(reconnected?.copiedCount, 1)
        XCTAssertEqual(reconnected?.sourceAvailable, true)

        let recoveryID = try await supervisor.requestOperation(
            method: "operation.start",
            parameters: RecoverOperationParameters(
                catalogPath: fixture.catalog.path,
                operationID: operationID
            )
        )
        XCTAssertEqual(recoveryID, operationID)
        let recoveryEvents = try await collectEvents(
            try await supervisor.events(for: recoveryID)
        )
        guard case .completed = recoveryEvents.last?.payload else {
            return XCTFail("Expected recovery to complete after explicit retry")
        }
        let completedInspection = try await supervisor.inspectUnfinishedOperation(
            catalogPath: fixture.catalog.path
        )
        XCTAssertNil(completedInspection)
        try verifyCatalog(
            fixture.catalog,
            collectionName: "Source Recovery Import",
            expectedFiles: 3
        )
        try await supervisor.stop()
    }

    func testRealEnginePreservesCompoundItemAndVerifiesOriginals() async throws {
        let fixture = try makeCompoundFixture()
        let supervisor = EngineSupervisor()
        _ = try await supervisor.start()
        addTeardownBlock {
            try? await supervisor.stop()
        }
        _ = try await supervisor.handshake()
        _ = try await supervisor.createCatalog(
            path: fixture.catalog.path,
            displayName: "Compound Catalog"
        )
        let review = try await plannedImport(
            supervisor: supervisor,
            fixture: fixture,
            collectionName: "Compound Import"
        )
        XCTAssertEqual(review.transferCount, 2)
        XCTAssertEqual(review.leftOnSourceCount, 0)
        XCTAssertEqual(review.scan.unsupportedCount, 0)

        let operationID = try await supervisor.requestOperation(
            method: "operation.start",
            parameters: StartImportParameters(
                reviewedPlan: try XCTUnwrap(review.reviewedPlan)
            )
        )
        let events = try await collectEvents(
            try await supervisor.events(for: operationID)
        )
        guard case .completed = events.last?.payload else {
            return XCTFail("Expected a completed terminal event")
        }

        try verifyCompoundCatalog(fixture.catalog)
        try await supervisor.stop()
    }

    func testRealEngineRoutesAndRecoversInterruptedImport() async throws {
        let fixture = try makeFixture(fileCount: 3, bytesPerFile: 32)
        let supervisor = EngineSupervisor(
            arguments: ["--test-progress-delay-ms", "2000"]
        )
        let processIdentifier = try await supervisor.start()
        addTeardownBlock {
            try? await supervisor.stop()
        }
        _ = try await supervisor.handshake()
        _ = try await supervisor.createCatalog(
            path: fixture.catalog.path,
            displayName: "Recovery Catalog"
        )
        let review = try await plannedImport(
            supervisor: supervisor,
            fixture: fixture,
            collectionName: "Recovery Import"
        )
        let operationID = try await supervisor.requestOperation(
            method: "operation.start",
            parameters: StartImportParameters(
                reviewedPlan: try XCTUnwrap(review.reviewedPlan)
            )
        )
        let stream = try await supervisor.events(for: operationID)
        var interrupted = false
        do {
            for try await event in stream {
                if case .progress(
                    completedUnits: 1,
                    totalUnits: _,
                    message: _
                ) = event.payload {
                    XCTAssertEqual(Darwin.kill(processIdentifier, SIGKILL), 0)
                    interrupted = true
                    break
                }
            }
        } catch {
            // The forced process interruption is the behavior under test.
        }
        XCTAssertTrue(interrupted)
        _ = try? await supervisor.waitForTermination()

        let recoverySupervisor = EngineSupervisor()
        _ = try await recoverySupervisor.start()
        addTeardownBlock {
            try? await recoverySupervisor.stop()
        }
        _ = try await recoverySupervisor.handshake()
        let unfinished = try await recoverySupervisor.inspectUnfinishedOperation(
            catalogPath: fixture.catalog.path
        )
        guard let unfinished else {
            return XCTFail("The engine completed before the interruption was observed")
        }
        XCTAssertEqual(unfinished.operationID, operationID)
        XCTAssertEqual(unfinished.collectionName, "Recovery Import")
        XCTAssertEqual(unfinished.copiedCount, 1)
        XCTAssertEqual(unfinished.totalCount, 3)
        XCTAssertEqual(unfinished.sourceAvailable, true)

        let recoveryID = try await recoverySupervisor.requestOperation(
            method: "operation.start",
            parameters: RecoverOperationParameters(
                catalogPath: fixture.catalog.path,
                operationID: operationID
            )
        )
        XCTAssertEqual(recoveryID, operationID)
        let recoveryEvents = try await collectEvents(
            try await recoverySupervisor.events(for: recoveryID)
        )
        XCTAssertEqual(
            recoveryEvents.map(\.sequence),
            Array(0..<UInt64(recoveryEvents.count))
        )
        guard case .completed = recoveryEvents.last?.payload else {
            try? await recoverySupervisor.stop()
            return XCTFail(
                "Expected recovery to complete, got \(String(describing: recoveryEvents.last?.payload))"
            )
        }

        let recoveredInspection =
            try await recoverySupervisor.inspectUnfinishedOperation(
                catalogPath: fixture.catalog.path
            )
        XCTAssertNil(recoveredInspection)
        try verifyCatalog(
            fixture.catalog,
            collectionName: "Recovery Import",
            expectedFiles: 3
        )
        try await recoverySupervisor.stop()
    }

    private func plannedImport(
        supervisor: EngineSupervisor,
        fixture: Fixture,
        collectionName: String
    ) async throws -> EngineSupervisor.ImportReview {
        let scan = try await supervisor.scanImport(sourcePath: fixture.source.path)
        XCTAssertEqual(scan.mediaCount, fixture.fileCount)
        return try await supervisor.reviewImport(
            catalogPath: fixture.catalog.path,
            sourcePath: fixture.source.path,
            collectionName: collectionName,
            destinationMode: .create,
            acceptedDate: "2026-10-10",
            dateResolution: .asShown
        )
    }

    private func collectEvents(
        _ stream: AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error>
    ) async throws -> [EngineSupervisor.OperationEvent] {
        var events: [EngineSupervisor.OperationEvent] = []
        for try await event in stream {
            events.append(event)
        }
        return events
    }

    private func makeFixture(
        fileCount: Int,
        bytesPerFile: Int
    ) throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "SouvenirsRealEngineSmoke-\(UUID().uuidString)",
                isDirectory: true
            )
        let catalog = root.appendingPathComponent("catalog", isDirectory: true)
        let source = root.appendingPathComponent("source", isDirectory: true)
        try FileManager.default.createDirectory(
            at: catalog,
            withIntermediateDirectories: true
        )
        try FileManager.default.createDirectory(
            at: source,
            withIntermediateDirectories: true
        )
        for index in 0..<fileCount {
            let contents = Data(
                repeating: UInt8(index % 251),
                count: bytesPerFile
            )
            try contents.write(
                to: source.appendingPathComponent(
                    String(format: "IMG_20261010_1011%02d.jpg", index)
                )
            )
        }
        temporaryDirectories.append(root)
        return Fixture(
            catalog: catalog,
            source: source,
            fileCount: fileCount
        )
    }

    private func makeCompoundFixture() throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "SouvenirsRealEngineCompound-\(UUID().uuidString)",
                isDirectory: true
            )
        let catalog = root.appendingPathComponent("catalog", isDirectory: true)
        let source = root.appendingPathComponent("source", isDirectory: true)
        try FileManager.default.createDirectory(
            at: catalog,
            withIntermediateDirectories: true
        )
        try FileManager.default.createDirectory(
            at: source,
            withIntermediateDirectories: true
        )
        try Data("raw bytes".utf8).write(
            to: source.appendingPathComponent("IMG_20261010_120000.CR3")
        )
        try Data("jpeg bytes".utf8).write(
            to: source.appendingPathComponent("IMG_20261010_120000.JPG")
        )
        try Data("xmp bytes".utf8).write(
            to: source.appendingPathComponent("IMG_20261010_120000.JPG.xmp")
        )
        temporaryDirectories.append(root)
        return Fixture(
            catalog: catalog,
            source: source,
            fileCount: 2
        )
    }

    private func verifyCatalog(
        _ catalog: URL,
        collectionName: String,
        expectedFiles: Int
    ) throws {
        let collection = catalog
            .appendingPathComponent("2026", isDirectory: true)
            .appendingPathComponent(
                "2026-10-10 \(collectionName)",
                isDirectory: true
            )
        let manifest = collection.appendingPathComponent("SHA256SUMS")
        let lines = try String(contentsOf: manifest, encoding: .utf8)
            .split(separator: "\n")
        XCTAssertEqual(lines.count, expectedFiles)
        let files = try FileManager.default.contentsOfDirectory(
            at: collection,
            includingPropertiesForKeys: nil
        ).filter { $0.lastPathComponent != "SHA256SUMS" }
        XCTAssertEqual(files.count, expectedFiles)

        let entries = Dictionary(uniqueKeysWithValues: try lines.map { line in
            guard let separator = line.range(of: "  ") else {
                throw SmokeError.invalidManifest
            }
            return (
                String(line[separator.upperBound...]),
                String(line[..<separator.lowerBound])
            )
        })
        for file in files {
            let digest = SHA256.hash(data: try Data(contentsOf: file))
                .map { String(format: "%02x", $0) }
                .joined()
            XCTAssertEqual(entries[file.lastPathComponent], digest)
        }
    }

    private func verifyCompoundCatalog(_ catalog: URL) throws {
        let collection = catalog
            .appendingPathComponent("2026", isDirectory: true)
            .appendingPathComponent(
                "2026-10-10 Compound Import",
                isDirectory: true
            )
        let expectedOriginals = [
            "2026-10-10 12-00-00 001.CR3",
            "2026-10-10 12-00-00 001.JPG",
        ]
        let sidecar = "2026-10-10 12-00-00 001.JPG.xmp"
        for filename in expectedOriginals + [sidecar] {
            XCTAssertTrue(
                FileManager.default.fileExists(
                    atPath: collection
                        .appendingPathComponent(filename)
                        .path
                )
            )
        }

        let manifest = collection.appendingPathComponent("SHA256SUMS")
        let lines = try String(contentsOf: manifest, encoding: .utf8)
            .split(separator: "\n")
        XCTAssertEqual(lines.count, expectedOriginals.count)
        XCTAssertFalse(lines.contains { $0.contains(".xmp") })
        let entries = Dictionary(uniqueKeysWithValues: try lines.map { line in
            guard let separator = line.range(of: "  ") else {
                throw SmokeError.invalidManifest
            }
            return (
                String(line[separator.upperBound...]),
                String(line[..<separator.lowerBound])
            )
        })
        for filename in expectedOriginals {
            let file = collection.appendingPathComponent(filename)
            let digest = SHA256.hash(data: try Data(contentsOf: file))
                .map { String(format: "%02x", $0) }
                .joined()
            XCTAssertEqual(entries[filename], digest)
        }
    }

    private func verifyCancelledJournal(
        _ catalog: URL,
        operationID: String
    ) throws {
        let journalDirectory = catalog
            .appendingPathComponent(".souvenirs", isDirectory: true)
            .appendingPathComponent("journal", isDirectory: true)
        let journal = try XCTUnwrap(
            FileManager.default.contentsOfDirectory(
                at: journalDirectory,
                includingPropertiesForKeys: nil
            ).first { $0.lastPathComponent.contains(operationID) }
        )
        let lines = try String(contentsOf: journal, encoding: .utf8)
            .split(separator: "\n")
        let lastLine = try XCTUnwrap(lines.last)
        let outcome = try JSONSerialization.jsonObject(
            with: Data(lastLine.utf8)
        ) as? [String: String]
        XCTAssertEqual(outcome?["kind"], "outcome")
        XCTAssertEqual(outcome?["outcome"], "cancelled")
    }
}

private struct Fixture {
    let catalog: URL
    let source: URL
    let fileCount: Int
}

private enum SmokeError: Error {
    case invalidManifest
}
