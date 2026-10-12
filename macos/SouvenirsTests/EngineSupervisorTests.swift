import Foundation
import Darwin
import XCTest
@testable import Souvenirs

final class EngineSupervisorTests: XCTestCase {
    private var fixtureDirectories: [URL] = []

    override func tearDownWithError() throws {
        for directory in fixtureDirectories {
            try? FileManager.default.removeItem(at: directory)
        }
        fixtureDirectories.removeAll()
        try? FileManager.default.removeItem(at: fixtureRoot)
        try super.tearDownWithError()
    }

    func testHandshakeSendsContractAndParsesResponse() async throws {
        let executable = try makeExecutable(
            """
            trap 'exit 99' TERM
            IFS= read -r request || exit 2
            expected='{"id":"macos-handshake-1","method":"handshake","params":{"client_version":"0.2.0"},"protocol_version":9}'
            if [ "$request" != "$expected" ]; then
                echo "unexpected request: $request" >&2
                exit 3
            fi
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.2.0","protocol_version":9,"compatible_client_version":"0.2.0","capabilities":["operation.cancel"]}}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)

        _ = try await supervisor.start()
        let handshake = try await supervisor.handshake()

        XCTAssertEqual(
            handshake,
            .init(
                engineVersion: "0.2.0",
                protocolVersion: 9,
                compatibleClientVersion: "0.2.0",
                capabilities: ["operation.cancel"]
            )
        )

        try await supervisor.stop()
        let termination = await supervisor.lastTermination
        let state = await supervisor.state
        XCTAssertEqual(termination, .init(reason: .exit, status: 0))
        XCTAssertEqual(state, .idle)
    }

    func testHandshakeRejectsProtocolError() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"kind":"response","id":null,"protocol_version":9,"status":"error","error":{"code":"invalid_request","message":"request could not be decoded"}}
                """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the protocol error to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(
                error,
                .protocolError(
                    code: "invalid_request",
                    message: "request could not be decoded"
                )
            )
        }

        try await supervisor.stop()
    }

    func testHandshakeRejectsPreviousProtocolVersion() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"kind":"response","id":"macos-handshake-1","protocol_version":1,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"0.1.0","capabilities":[]}}
                """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the incompatible protocol to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .incompatibleProtocol(expected: 9, actual: 1))
        }

        try await supervisor.stop()
    }

    func testHandshakeRejectsPreviousVersionReportedByCurrentEnvelope() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"0.1.0","capabilities":[]}}
                """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the incompatible protocol to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .incompatibleProtocol(expected: 9, actual: 1))
        }

        try await supervisor.stop()
    }

    func testHandshakeRejectsFutureEnvelopeBeforeTypedFields() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"id":"macos-handshake-1","protocol_version":10,"future_result":{}}
                """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the incompatible protocol to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .incompatibleProtocol(expected: 9, actual: 10))
        }

        try await supervisor.stop()
    }

    func testHandshakeAcceptsNewerClientVersion() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":9,"compatible_client_version":"0.0.10","capabilities":[]}}
                """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        let handshake = try await supervisor.handshake()

        XCTAssertEqual(handshake.compatibleClientVersion, "0.0.10")
        try await supervisor.stop()
    }

    func testHandshakeRejectsClientOlderThanMinimumVersion() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.2.0","protocol_version":9,"compatible_client_version":"0.2.0","capabilities":[]}}
                """
        )
        let supervisor = EngineSupervisor(
            executableURL: executable,
            handshakeTimeout: .seconds(3),
            shutdownTimeout: .seconds(3),
            terminationTimeout: .seconds(1),
            clientVersion: "0.1.0"
        )
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the incompatible client version to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(
                error,
                .incompatibleClientVersion(expected: "0.1.0", actual: "0.2.0")
            )
        }

        try await supervisor.stop()
    }

    func testHandshakeRejectsMalformedMinimumClientVersion() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":9,"compatible_client_version":"0.01.0","capabilities":[]}}
                """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the malformed client version to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(
                error,
                .malformedResponse(
                    "The compatible_client_version is not a valid major.minor.patch version"
                )
            )
        }

        try await supervisor.stop()
    }

    func testClientVersionComparisonUsesNumericComponents() throws {
        let older = try XCTUnwrap(ClientVersion("0.9.10"))
        let newer = try XCTUnwrap(ClientVersion("0.10.0"))
        let patch = try XCTUnwrap(ClientVersion("0.10.1"))

        XCTAssertLessThan(older, newer)
        XCTAssertLessThan(newer, patch)
        XCTAssertEqual(
            ClientVersion("1.2.3"),
            ClientVersion("1.2.3")
        )
    }

    func testClientVersionRejectsMalformedSyntax() {
        for version in [
            "",
            "1",
            "1.2",
            "1.2.3.4",
            "1.02.3",
            "1.a.3",
            "-1.2.3",
            "18446744073709551616.0.0",
        ] {
            XCTAssertNil(ClientVersion(version), "\(version) should be rejected")
        }
    }

    func testHandshakeParsesSharedGoldenFixture() async throws {
        let executable = try makeHandshakeFixture(
            response: try sharedHandshakeResponse()
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        let handshake = try await supervisor.handshake()

        XCTAssertEqual(handshake.engineVersion, "0.1.0")
        XCTAssertEqual(handshake.compatibleClientVersion, "0.2.0")
        XCTAssertEqual(
            handshake.capabilities,
            [
                "operation.cancel",
                "catalog.browse",
                "catalog.collections",
                "import.review",
            ]
        )
        try await supervisor.stop()
    }

    func testCatalogCollectionQueryDecodesExactIdentities() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.2.0","protocol_version":9,"compatible_client_version":"0.2.0","capabilities":["catalog.collections"]}}'
            IFS= read -r request || exit 3
            id=$(printf '%s' "$request" | sed -E 's/.*"id":"([^"]+)".*/\\1/')
            printf '%s\\n' '{"kind":"response","id":"'"$id"'","protocol_version":9,"status":"success","result":{"type":"catalog_collections","collections":[{"kind":"dated","identity":{"kind":"dated","year":"2019","directory":"2019-07-14 +6d Brittany"},"label":"Brittany","start_date":"2019-07-14","end_date":"2019-07-20"},{"kind":"ongoing","identity":{"kind":"ongoing","key":"kids"},"label":"Kids","parts":[{"year":2019,"year_directory":"2019","collection_directory":"z Kids"}]}]}}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()

        let collections = try await supervisor.catalogCollections(path: "/catalog")

        XCTAssertEqual(
            collections,
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
                        ),
                    ]
                ),
            ]
        )
        try await supervisor.stop()
    }

    func testCatalogBrowseQueryDecodesMediaAndAnnualParts() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.2.0","protocol_version":9,"compatible_client_version":"0.2.0","capabilities":["catalog.browse"]}}'
            IFS= read -r request || exit 3
            id=$(printf '%s' "$request" | sed -E 's/.*"id":"([^"]+)".*/\\1/')
            printf '%s\\n' '{"kind":"response","id":"'"$id"'","protocol_version":9,"status":"success","result":{"type":"catalog_browse","navigation":[{"kind":"all_photos"},{"kind":"year","year":2019,"entries":[{"identity":{"kind":"dated","year":"2019","directory":"2019-07-14 +6d Brittany"},"annual_part_year":null},{"identity":{"kind":"ongoing","key":"kids"},"annual_part_year":2019}]}],"collections":[{"kind":"dated","identity":{"kind":"dated","year":"2019","directory":"2019-07-14 +6d Brittany"},"label":"Brittany","start_date":"2019-07-14","end_date":"2019-07-20","media_count":1,"date_groups":[{"date":"2019-07-15","media":[{"filename":"2019-07-15 10-11-12 001.jpg","path":"/catalog/2019/2019-07-14 +6d Brittany/2019-07-15 10-11-12 001.jpg"}]}]},{"kind":"ongoing","identity":{"kind":"ongoing","key":"kids"},"label":"Kids","media_count":1,"parts":[{"year":2019,"year_directory":"2019","collection_directory":"z Kids","media_count":1,"date_groups":[{"date":"2019-03-02","media":[{"filename":"2019-03-02 10-11-05 001.heic","path":"/catalog/2019/z Kids/2019-03-02 10-11-05 001.heic"}]}]}]}]}}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()

        let snapshot = try await supervisor.catalogBrowse(path: "/catalog")
        XCTAssertEqual(snapshot.navigation.count, 2)

        guard case .dated(
            identity: .dated(year: "2019", directory: "2019-07-14 +6d Brittany"),
            label: "Brittany",
            startDate: "2019-07-14",
            endDate: "2019-07-20",
            mediaCount: 1,
            coverMedia: _,
            dateGroups: let datedGroups,
            problem: nil
        ) = snapshot.collections[0] else {
            return XCTFail("Expected dated browse collection")
        }
        XCTAssertEqual(datedGroups[0].date, "2019-07-15")
        XCTAssertEqual(datedGroups[0].media[0].filename, "2019-07-15 10-11-12 001.jpg")

        guard case .ongoing(
            identity: .ongoing(key: "kids"),
            label: "Kids",
            mediaCount: 1,
            parts: let parts
        ) = snapshot.collections[1] else {
            return XCTFail("Expected ongoing browse collection")
        }
        XCTAssertEqual(parts[0].yearDirectory, "2019")
        XCTAssertEqual(parts[0].dateGroups[0].media[0].path, "/catalog/2019/z Kids/2019-03-02 10-11-05 001.heic")
        try await supervisor.stop()
    }

    func testCurrentClientDecodesDateLessScanAfterHandshake() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.2.0","protocol_version":9,"compatible_client_version":"0.2.0","capabilities":["import.scan"]}}'
            IFS= read -r request || exit 3
            id=$(printf '%s' "$request" | sed -E 's/.*"id":"([^"]+)".*/\\1/')
            printf '%s\\n' '{"kind":"response","id":"'"$id"'","protocol_version":9,"status":"success","result":{"type":"import_scan","source_path":"/source","source_name":"source","file_count":1,"media_count":1,"photo_count":1,"video_count":0,"unsupported_count":0,"uncertain_time_count":0,"total_bytes":3,"suggested_collection_name":"Source","suggested_accepted_date":null,"suggested_range_end":null,"pending_item_count":0,"unresolved_item_count":1,"datetime_sources":[]}}'; while IFS= read -r _; do :; done
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        let handshake = try await supervisor.handshake()
        let scan = try await supervisor.scanImport(sourcePath: "/source")
        XCTAssertEqual(handshake.compatibleClientVersion, "0.2.0")
        XCTAssertNil(scan.suggestedAcceptedDate)
        XCTAssertEqual(scan.unresolvedItemCount, 1)
        try await supervisor.stop()
    }

    func testCurrentClientDecodesImportReviewEvidence() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.2.0","protocol_version":9,"compatible_client_version":"0.2.0","capabilities":["import.review"]}}'
            IFS= read -r request || exit 3
            id=$(printf '%s' "$request" | sed -E 's/.*"id":"([^"]+)".*/\\1/')
            printf '%s\\n' '{"kind":"response","id":"'"$id"'","protocol_version":9,"status":"success","result":{"type":"import_review","scan":{"source_path":"/source","source_name":"source","file_count":1,"media_count":1,"photo_count":1,"video_count":0,"unsupported_count":0,"uncertain_time_count":0,"total_bytes":3,"suggested_collection_name":"Source","suggested_accepted_date":"2026-10-10","suggested_range_end":"2026-10-10","pending_item_count":0,"unresolved_item_count":0,"datetime_sources":[]},"collection_name":"Trip","destination":{"kind":"create","collection_name":"Trip"},"destination_path":"/catalog/2026/2026-10-10 Trip","accepted_date":"2026-10-10","date_resolution":"as_shown","required_decision_count":0,"transfer_count":1,"skipped_count":0,"left_on_source_count":0,"operation_count":3,"preflight":{"representative_file_names":["2026-10-10 10-11-12 001.jpg"],"required_bytes":3,"available_bytes":1024,"available_space":"passed","write_probe":"passed","portable_names":"passed","destination_conflicts":"passed"},"can_start":true,"reviewed_plan":"opaque"}}'
            IFS= read -r request || exit 4
            id=$(printf '%s' "$request" | sed -E 's/.*"id":"([^"]+)".*/\\1/')
            printf '%s\\n' '{"kind":"response","id":"'"$id"'","protocol_version":9,"status":"success","result":{"type":"import_review","scan":{"source_path":"/source","source_name":"source","file_count":1,"media_count":1,"photo_count":1,"video_count":0,"unsupported_count":0,"uncertain_time_count":1,"total_bytes":3,"suggested_collection_name":"Source","suggested_accepted_date":null,"suggested_range_end":null,"pending_item_count":1,"unresolved_item_count":0,"datetime_sources":[]},"collection_name":"Trip","destination":{"kind":"create","collection_name":"Trip"},"destination_path":"/catalog/2026/2026-10-10 Trip","accepted_date":"2026-10-10","date_resolution":null,"required_decision_count":1,"transfer_count":0,"skipped_count":1,"left_on_source_count":1,"operation_count":0,"preflight":{"representative_file_names":[],"required_bytes":0,"available_bytes":null,"available_space":"not_run","write_probe":"not_run","portable_names":"not_run","destination_conflicts":"not_run"},"can_start":false,"reviewed_plan":null}}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()

        let review = try await supervisor.reviewImport(
            catalogPath: "/catalog",
            sourcePath: "/source",
            destination: .create(collectionName: "Trip"),
            dateResolution: .asShown
        )

        XCTAssertEqual(
            review.preflight.representativeFileNames,
            ["2026-10-10 10-11-12 001.jpg"]
        )
        XCTAssertEqual(review.preflight.requiredBytes, 3)
        XCTAssertEqual(review.preflight.availableBytes, 1024)
        XCTAssertEqual(review.preflight.availableSpace, .passed)
        XCTAssertEqual(review.preflight.writeProbe, .passed)
        XCTAssertEqual(review.preflight.portableNames, .passed)
        XCTAssertEqual(review.preflight.destinationConflicts, .passed)

        let notRun = try await supervisor.reviewImport(
            catalogPath: "/catalog",
            sourcePath: "/source",
            destination: .create(collectionName: "Trip"),
            dateResolution: nil
        )
        XCTAssertNil(notRun.preflight.availableBytes)
        XCTAssertEqual(notRun.preflight.availableSpace, .notRun)
        XCTAssertEqual(notRun.preflight.writeProbe, .notRun)
        XCTAssertEqual(notRun.preflight.portableNames, .notRun)
        XCTAssertEqual(notRun.preflight.destinationConflicts, .notRun)
        try await supervisor.stop()
    }

    func testHandshakeRejectsMalformedResponse() async throws {
        let executable = try makeHandshakeFixture(response: "{not-json}")
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the malformed response to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            guard case .malformedResponse = error else {
                return XCTFail("Unexpected supervisor error: \(error)")
            }
        }

        try await supervisor.stop()
    }

    func testHandshakeReportsEOFWhenEngineCrashes() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            exit 17
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the engine termination to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(
                error,
                .unexpectedTermination(.init(reason: .exit, status: 17))
            )
        }

        let termination = try await supervisor.waitForTermination()
        let state = await supervisor.state
        XCTAssertEqual(termination, .init(reason: .exit, status: 17))
        XCTAssertEqual(state, .exitedUnexpectedly(termination))
    }

    func testConcurrentRequestsCorrelateOutOfOrderResponses() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r first || exit 3
            IFS= read -r second || exit 4
            printf '%s\\n' '{"kind":"response","id":"macos-2","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-second"}}'
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-first"}}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()

        let first = Task {
            try await supervisor.requestOperation(
                method: "test.first",
                parameters: TestParameters(value: "first")
            )
        }
        try await Task.sleep(for: .milliseconds(50))
        let second = Task {
            try await supervisor.requestOperation(
                method: "test.second",
                parameters: TestParameters(value: "second")
            )
        }

        let secondOperation = try await second.value
        let firstOperation = try await first.value
        XCTAssertEqual(firstOperation, "operation-first")
        XCTAssertEqual(secondOperation, "operation-second")
        try await supervisor.stop()
    }

    func testOperationEventsPreserveOrderWhenBufferedBeforeSubscription() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":1,"event":"progress","completed_units":1,"total_units":2,"message":"Working"}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":2,"event":"completed","result":{"item_count":1}}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()
        let operationID = try await supervisor.requestOperation(
            method: "test.operation",
            parameters: TestParameters(value: "events")
        )
        try await Task.sleep(for: .milliseconds(50))

        let events = try await collect(
            try await supervisor.events(for: operationID)
        )

        XCTAssertEqual(events.map(\.sequence), [0, 1, 2])
        XCTAssertEqual(events.first?.payload, .started(name: "test.operation"))
        XCTAssertTrue(events.last?.isTerminal == true)
        try await supervisor.stop()
    }

    func testOutOfOrderOperationEventFailsStream() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":2,"event":"cancelled","copied_count":1}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()
        let operationID = try await supervisor.requestOperation(
            method: "test.operation",
            parameters: TestParameters(value: "out-of-order")
        )
        try await Task.sleep(for: .milliseconds(50))

        do {
            _ = try await collect(try await supervisor.events(for: operationID))
            XCTFail("Expected the event stream to reject the sequence gap")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(
                error,
                .outOfOrderEvent(
                    operationID: "operation-1",
                    expected: 1,
                    actual: 2
                )
            )
        }
        try await supervisor.stop()
    }

    func testCancelledEventRejectsNegativeCopiedCount() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":0,"event":"cancelled","copied_count":-1}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()
        let operationID = try await supervisor.requestOperation(
            method: "test.operation",
            parameters: TestParameters(value: "negative-copied-count")
        )

        do {
            _ = try await collect(try await supervisor.events(for: operationID))
            XCTFail("Expected the event stream to reject a negative copied count")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(
                error,
                .malformedResponse(
                    "A cancelled event requires a nonnegative copied_count"
                )
            )
        }
        try await supervisor.stop()
    }

    func testTerminalEventIsDeliveredWhenEngineExitsImmediately() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":0,"event":"completed","result":{"item_count":1}}'
            exit 0
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()
        let operationID = try await supervisor.requestOperation(
            method: "test.operation",
            parameters: TestParameters(value: "exit")
        )

        let events = try await collect(
            try await supervisor.events(for: operationID)
        )

        XCTAssertEqual(events.map(\.sequence), [0])
        XCTAssertTrue(events[0].isTerminal)
        let termination = try await supervisor.waitForTermination()
        XCTAssertEqual(termination, .init(reason: .exit, status: 0))
    }

    func testBufferedNonterminalEventFailsWhenEngineExitsImmediately() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
            exit 37
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()
        let operationID = try await supervisor.requestOperation(
            method: "test.operation",
            parameters: TestParameters(value: "exit")
        )
        let result = try await collectWithinTimeout(
            try await supervisor.events(for: operationID)
        )

        XCTAssertEqual(result.events.map(\.sequence), [0])
        XCTAssertEqual(
            result.events.first?.payload,
            .started(name: "test.operation")
        )
        XCTAssertEqual(
            result.failure,
            .unexpectedTermination(.init(reason: .exit, status: 37))
        )
    }

    func testStandardOutputEOFFailsPendingRequestAndActiveOperationStream() async throws {
        let executable = try makeExecutable(
            """
            trap 'exit 0' TERM
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
            IFS= read -r _ || exit 4
            exec 1>&-
            sleep 5
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()
        let operationID = try await supervisor.requestOperation(
            method: "test.operation",
            parameters: TestParameters(value: "stdout-eof")
        )
        let stream = try await supervisor.events(for: operationID)
        let cancellation = Task {
            try await supervisor.cancel(operationID: operationID)
        }

        do {
            _ = try await cancellation.value
            XCTFail("Expected the pending request to fail at stdout EOF")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .responseStreamEnded)
        }

        let result = try await collectWithinTimeout(stream)
        XCTAssertEqual(result.events.map(\.sequence), [0])
        XCTAssertEqual(result.failure, .responseStreamEnded)

        _ = try? await supervisor.stop()
        let termination = try await supervisor.waitForTermination()
        XCTAssertEqual(termination, .init(reason: .exit, status: 0))
    }

    func testStandardOutputEOFFailsBufferedOperationStream() async throws {
        let executable = try makeExecutable(
            """
            trap 'exit 0' TERM
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
            exec 1>&-
            sleep 5
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()
        let operationID = try await supervisor.requestOperation(
            method: "test.operation",
            parameters: TestParameters(value: "stdout-eof")
        )
        do {
            _ = try await supervisor.cancel(operationID: operationID)
            XCTFail("Expected stdout EOF to make the connection terminal")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .responseStreamEnded)
        }

        let result = try await collectWithinTimeout(
            try await supervisor.events(for: operationID)
        )

        XCTAssertEqual(result.events.map(\.sequence), [0])
        XCTAssertEqual(result.failure, .responseStreamEnded)
        _ = try? await supervisor.stop()
    }

    func testTerminalEventRemainsSuccessfulWhenStandardOutputCloses() async throws {
        let executable = try makeExecutable(
            """
            trap 'exit 0' TERM
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":0,"event":"completed","result":{"item_count":1}}'
            exec 1>&-
            sleep 5
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()
        let operationID = try await supervisor.requestOperation(
            method: "test.operation",
            parameters: TestParameters(value: "stdout-eof")
        )

        let result = try await collectWithinTimeout(
            try await supervisor.events(for: operationID)
        )

        XCTAssertEqual(result.events.map(\.sequence), [0])
        XCTAssertTrue(result.events[0].isTerminal)
        XCTAssertNil(result.failure)
        _ = try? await supervisor.stop()
    }

    func testLateResponseDoesNotFailAnotherRequest() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            sleep 0.7
            IFS= read -r _ || exit 4
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-late"}}'
            printf '%s\\n' '{"kind":"response","id":"macos-2","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-current"}}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()

        do {
            _ = try await supervisor.requestOperation(
                method: "test.timeout",
                parameters: TestParameters(value: "late")
            )
            XCTFail("Expected the first request to time out")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .requestTimedOut("macos-1"))
        }

        let operationID = try await supervisor.requestOperation(
            method: "test.current",
            parameters: TestParameters(value: "current")
        )
        XCTAssertEqual(operationID, "operation-current")
        try await supervisor.stop()
    }

    func testCompletionCanWinCancellationRace() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":9,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
            IFS= read -r _ || exit 4
            printf '%s\\n' '{"kind":"event","protocol_version":9,"operation_id":"operation-1","sequence":1,"event":"completed","result":{"won":"completion"}}'
            printf '%s\\n' '{"kind":"response","id":"macos-2","protocol_version":9,"status":"success","result":{"type":"cancellation_accepted","operation_id":"operation-1"}}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()
        let operationID = try await supervisor.requestOperation(
            method: "test.operation",
            parameters: TestParameters(value: "cancel")
        )
        let stream = try await supervisor.events(for: operationID)

        async let events = collect(stream)
        let cancellation = try await supervisor.cancel(operationID: operationID)

        XCTAssertEqual(cancellation.operationID, operationID)
        let received = try await events
        XCTAssertEqual(received.map(\.sequence), [0, 1])
        XCTAssertTrue(received.last?.isTerminal == true)
        try await supervisor.stop()
    }

    func testUnexpectedTerminationFailsPendingRequest() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            exit 29
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()

        do {
            _ = try await supervisor.requestOperation(
                method: "test.crash",
                parameters: TestParameters(value: "crash")
            )
            XCTFail("Expected the pending request to fail")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(
                error,
                .unexpectedTermination(.init(reason: .exit, status: 29))
            )
        }
    }

    func testTerminationDoesNotWaitForeverForInheritedStandardOutput() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            (sleep 5) &
            exit 31
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()

        let termination = try await withThrowingTaskGroup(
            of: EngineSupervisor.Termination.self
        ) { group in
            group.addTask {
                try await supervisor.waitForTermination()
            }
            group.addTask {
                try await Task.sleep(for: .seconds(1))
                throw TestTimeout()
            }
            let result = try await group.next()
            group.cancelAll()
            return try XCTUnwrap(result)
        }

        XCTAssertEqual(termination, .init(reason: .exit, status: 31))
        let state = await supervisor.state
        XCTAssertEqual(state, .exitedUnexpectedly(termination))
    }

    func testHandshakeTimesOutWhenEngineIsSilent() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the handshake to time out")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .handshakeTimedOut)
        }

        try await supervisor.stop()
    }

    func testStopClosesStandardInputAndWaitsForCleanExit() async throws {
        let executable = try makeExecutable(
            """
            trap 'exit 99' TERM
            while IFS= read -r _; do :; done
            exit 0
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        try await supervisor.stop()

        let state = await supervisor.state
        let termination = await supervisor.lastTermination
        XCTAssertEqual(state, .idle)
        XCTAssertEqual(termination, .init(reason: .exit, status: 0))
    }

    func testStopTerminatesAfterEOFTimeout() async throws {
        let executable = try makeExecutable(
            """
            trap 'exit 23' TERM
            IFS= read -r _ || exit 2
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":9,"compatible_client_version":"0.1.0","capabilities":[]}}'
            while :; do :; done
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()

        do {
            try await supervisor.stop()
            XCTFail("Expected the clean shutdown to time out")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .shutdownTimedOut)
        }

        let state = await supervisor.state
        let termination = await supervisor.lastTermination
        XCTAssertEqual(state, .idle)
        XCTAssertEqual(termination, .init(reason: .exit, status: 23))
    }

    func testStopForceKillsAfterTerminateTimeout() async throws {
        let executable = try makeExecutable(
            """
            trap '' TERM
            IFS= read -r _ || exit 2
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":9,"compatible_client_version":"0.1.0","capabilities":[]}}'
            while :; do :; done
            """
        )
        let supervisor = makeSupervisor(executableURL: executable)
        _ = try await supervisor.start()
        _ = try await supervisor.handshake()

        do {
            try await supervisor.stop()
            XCTFail("Expected the clean shutdown to time out")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .shutdownTimedOut)
        }

        let state = await supervisor.state
        let termination = await supervisor.lastTermination
        XCTAssertEqual(state, .idle)
        XCTAssertEqual(
            termination,
            .init(reason: .uncaughtSignal, status: SIGKILL)
        )
    }

    func testStopReturnsWhenProcessAlreadyFinished() async throws {
        let executable = try makeExecutable("exit 0")
        let supervisor = EngineSupervisor(
            executableURL: executable,
            isProcessRunning: { _ in false },
            processTermination: { _ in
                .init(reason: .exit, status: 0)
            }
        )
        _ = try await supervisor.start()

        try await supervisor.stop()

        let state = await supervisor.state
        let termination = await supervisor.lastTermination
        XCTAssertEqual(state, .idle)
        XCTAssertEqual(termination, .init(reason: .exit, status: 0))
    }

    func testDuplicateStartIsRejected() async throws {
        let executable = try makeExecutable(
            """
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        let processIdentifier = try await supervisor.start()

        do {
            _ = try await supervisor.start()
            XCTFail("Expected a duplicate start to fail")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(
                error,
                .alreadyStarted(.running(processIdentifier: processIdentifier))
            )
        }

        try await supervisor.stop()
    }

    func testUnexpectedExitIsObserved() async throws {
        let executable = try makeExecutable("exit 17")
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        let termination = try await supervisor.waitForTermination()
        let state = await supervisor.state

        XCTAssertEqual(termination, .init(reason: .exit, status: 17))
        XCTAssertEqual(state, .exitedUnexpectedly(termination))
    }

    func testStartPassesConfiguredProcessArguments() async throws {
        let executable = try makeExecutable(
            """
            test "$1" = "--scenario" || exit 31
            test "$2" = "successful-operation" || exit 32
            """
        )
        let supervisor = EngineSupervisor(
            executableURL: executable,
            arguments: ["--scenario", "successful-operation"]
        )
        _ = try await supervisor.start()

        let termination = try await supervisor.waitForTermination()

        XCTAssertEqual(termination, .init(reason: .exit, status: 0))
    }

    @MainActor
    func testFakeEngineConfigurationRequiresExecutableAndScenario() {
        let executable = "/tmp/souvenirs-fake-engine"
        let configuration = EngineAppModel.fakeEngineConfiguration(
            environment: [
                "SOUVENIRS_ENGINE_EXECUTABLE": executable,
                "SOUVENIRS_FAKE_ENGINE_SCENARIO": "successful-operation",
            ]
        )

        XCTAssertEqual(
            configuration,
            EngineLaunchConfiguration(
                executableURL: URL(fileURLWithPath: executable),
                arguments: ["--scenario", "successful-operation"]
            )
        )
        XCTAssertNil(
            EngineAppModel.fakeEngineConfiguration(
                environment: ["SOUVENIRS_ENGINE_EXECUTABLE": executable]
            )
        )
        XCTAssertNil(
            EngineAppModel.fakeEngineConfiguration(
                environment: [
                    "SOUVENIRS_FAKE_ENGINE_SCENARIO": "successful-operation"
                ]
            )
        )
    }

    @MainActor
    func testEngineAppModelRetriesAfterStartupFailure() async throws {
        let executable = try makeExecutable("sleep 0.1\nexit 17")
        let model = EngineAppModel(
            environment: [
                "SOUVENIRS_ENGINE_EXECUTABLE": executable.path,
                "SOUVENIRS_FAKE_ENGINE_SCENARIO": "startup-failure",
            ]
        )

        model.start()
        await waitForEngineFailure(model)

        model.retry()
        XCTAssertEqual(model.status, .connecting)
        await waitForEngineFailure(model)
    }

    func testTerminationHandlerUsesInjectedMapper() async throws {
        let executable = try makeExecutable("exit 17")
        let mappedTermination = EngineSupervisor.Termination(
            reason: .exit,
            status: 41
        )
        let supervisor = EngineSupervisor(
            executableURL: executable,
            isProcessRunning: { $0.isRunning },
            processTermination: { _ in mappedTermination }
        )
        _ = try await supervisor.start()

        let termination = try await supervisor.waitForTermination()
        let state = await supervisor.state

        XCTAssertEqual(termination, mappedTermination)
        XCTAssertEqual(state, .exitedUnexpectedly(mappedTermination))
    }

    func testMissingExecutableReturnsExplicitError() async {
        let missingExecutable = fixtureRoot
            .appendingPathComponent(UUID().uuidString)
            .appendingPathComponent("missing-engine")
        let supervisor = EngineSupervisor(executableURL: missingExecutable)

        do {
            _ = try await supervisor.start()
            XCTFail("Expected a missing executable to fail")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .executableNotFound(missingExecutable))
        } catch {
            XCTFail("Unexpected error: \(error)")
        }

        let state = await supervisor.state
        XCTAssertEqual(state, .idle)
    }

    private lazy var fixtureRoot: URL = {
        FileManager.default.temporaryDirectory
            .appendingPathComponent(
                "EngineSupervisorTests-\(UUID().uuidString)",
                isDirectory: true
            )
    }()

    private let handshakeResponse =
        """
        {"kind":"response","id":"macos-handshake-1","protocol_version":9,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":9,"compatible_client_version":"0.1.0","capabilities":["operation.cancel"]}}
        """

    private func makeSupervisor(executableURL: URL) -> EngineSupervisor {
        EngineSupervisor(
            executableURL: executableURL,
            handshakeTimeout: .seconds(1),
            requestTimeout: .milliseconds(500),
            shutdownTimeout: .milliseconds(100),
            terminationTimeout: .milliseconds(100)
        )
    }

    private func makeHandshakeFixture(response: String) throws -> URL {
        try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(response)'
            while IFS= read -r _; do :; done
            """
        )
    }

    private func sharedHandshakeResponse() throws -> String {
        let fixture = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("crates/souvenirs-protocol/fixtures/protocol-v9.jsonl")
        let lines = try String(contentsOf: fixture, encoding: .utf8)
            .split(separator: "\n")
        guard let response = lines.first(where: {
            $0.contains(#""kind":"response""#)
                && $0.contains(#""type":"handshake""#)
        }) else {
            throw CocoaError(.fileReadCorruptFile)
        }
        return String(response)
    }

    private func makeExecutable(_ body: String) throws -> URL {
        let directory = fixtureRoot.appendingPathComponent(
            UUID().uuidString,
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        fixtureDirectories.append(directory)

        let executable = directory.appendingPathComponent("fixture-engine")
        try "#!/bin/sh\n\(body)\n".write(
            to: executable,
            atomically: true,
            encoding: .utf8
        )
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o700],
            ofItemAtPath: executable.path
        )
        return executable
    }

    @MainActor
    private func waitForEngineFailure(_ model: EngineAppModel) async {
        for _ in 0..<100 {
            if case .failed = model.status {
                return
            }
            try? await Task.sleep(for: .milliseconds(10))
        }
        XCTFail("Expected the engine model to report startup failure")
    }

    private func collect(
        _ stream: AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error>
    ) async throws -> [EngineSupervisor.OperationEvent] {
        var events: [EngineSupervisor.OperationEvent] = []
        for try await event in stream {
            events.append(event)
        }
        return events
    }

    private func collectWithinTimeout(
        _ stream: AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error>
    ) async throws -> StreamCollection {
        try await withThrowingTaskGroup(of: StreamCollection.self) { group in
            group.addTask {
                var events: [EngineSupervisor.OperationEvent] = []
                do {
                    for try await event in stream {
                        events.append(event)
                    }
                    return StreamCollection(events: events, failure: nil)
                } catch let error as EngineSupervisor.SupervisorError {
                    return StreamCollection(events: events, failure: error)
                }
            }
            group.addTask {
                try await Task.sleep(for: .seconds(1))
                throw TestTimeout()
            }
            let result = try await group.next()
            group.cancelAll()
            return try XCTUnwrap(result)
        }
    }
}

private struct TestParameters: Encodable, Sendable {
    let value: String
}

private struct StreamCollection: Sendable {
    let events: [EngineSupervisor.OperationEvent]
    let failure: EngineSupervisor.SupervisorError?
}

private struct TestTimeout: Error {}
