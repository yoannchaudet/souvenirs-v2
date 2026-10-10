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
            expected='{"id":"macos-handshake-1","method":"handshake","params":{"client_version":"0.1.0"},"protocol_version":1}'
            if [ "$request" != "$expected" ]; then
                echo "unexpected request: $request" >&2
                exit 3
            fi
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":1,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"0.1.0","capabilities":["operation.cancel"]}}'
            while IFS= read -r _; do :; done
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)

        _ = try await supervisor.start()
        let handshake = try await supervisor.handshake()

        XCTAssertEqual(
            handshake,
            .init(
                engineVersion: "0.1.0",
                protocolVersion: 1,
                compatibleClientVersion: "0.1.0",
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
                {"kind":"response","id":null,"protocol_version":1,"status":"error","error":{"code":"invalid_request","message":"request could not be decoded"}}
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

    func testHandshakeRejectsIncompatibleResponseVersion() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"kind":"response","id":"macos-handshake-1","protocol_version":2,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"0.1.0","capabilities":[]}}
                """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the incompatible protocol to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .incompatibleProtocol(expected: 1, actual: 2))
        }

        try await supervisor.stop()
    }

    func testHandshakeRejectsFutureEnvelopeBeforeTypedFields() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"id":"macos-handshake-1","protocol_version":2,"future_result":{}}
                """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        do {
            _ = try await supervisor.handshake()
            XCTFail("Expected the incompatible protocol to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .incompatibleProtocol(expected: 1, actual: 2))
        }

        try await supervisor.stop()
    }

    func testHandshakeAcceptsNewerClientVersion() async throws {
        let executable = try makeHandshakeFixture(
            response:
                """
                {"kind":"response","id":"macos-handshake-1","protocol_version":1,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"0.0.10","capabilities":[]}}
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
                {"kind":"response","id":"macos-handshake-1","protocol_version":1,"status":"success","result":{"type":"handshake","engine_version":"0.2.0","protocol_version":1,"compatible_client_version":"0.2.0","capabilities":[]}}
                """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
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
                {"kind":"response","id":"macos-handshake-1","protocol_version":1,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"0.01.0","capabilities":[]}}
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
        XCTAssertEqual(handshake.compatibleClientVersion, "0.1.0")
        XCTAssertEqual(handshake.capabilities, ["operation.cancel"])
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
            printf '%s\\n' '{"kind":"response","id":"macos-2","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-second"}}'
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-first"}}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":1,"event":"progress","completed_units":1,"total_units":2,"message":"Working"}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":2,"event":"completed","result":{"item_count":1}}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":2,"event":"cancelled"}'
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

    func testTerminalEventIsDeliveredWhenEngineExitsImmediately() async throws {
        let executable = try makeExecutable(
            """
            IFS= read -r _ || exit 2
            printf '%s\\n' '\(handshakeResponse)'
            IFS= read -r _ || exit 3
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":0,"event":"completed","result":{"item_count":1}}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":0,"event":"completed","result":{"item_count":1}}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-late"}}'
            printf '%s\\n' '{"kind":"response","id":"macos-2","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-current"}}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-1","protocol_version":1,"status":"success","result":{"type":"operation_accepted","operation_id":"operation-1"}}'
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":0,"event":"started","name":"test.operation"}'
            IFS= read -r _ || exit 4
            printf '%s\\n' '{"kind":"event","protocol_version":1,"operation_id":"operation-1","sequence":1,"event":"completed","result":{"won":"completion"}}'
            printf '%s\\n' '{"kind":"response","id":"macos-2","protocol_version":1,"status":"success","result":{"type":"cancellation_accepted","operation_id":"operation-1"}}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":1,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"0.1.0","capabilities":[]}}'
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
            printf '%s\\n' '{"kind":"response","id":"macos-handshake-1","protocol_version":1,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"0.1.0","capabilities":[]}}'
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
        {"kind":"response","id":"macos-handshake-1","protocol_version":1,"status":"success","result":{"type":"handshake","engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"0.1.0","capabilities":["operation.cancel"]}}
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
            .appendingPathComponent("crates/souvenirs-protocol/fixtures/protocol-v1.jsonl")
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
