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
            expected='{"id":"macos-handshake-1","method":"handshake","params":{},"protocol_version":1}'
            if [ "$request" != "$expected" ]; then
                echo "unexpected request: $request" >&2
                exit 3
            fi
            printf '%s\\n' '{"id":"macos-handshake-1","protocol_version":1,"result":{"engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"1","capabilities":["import"]}}'
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
                compatibleClientVersion: "1",
                capabilities: ["import"]
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
                {"id":null,"protocol_version":1,"error":{"code":"invalid_request","message":"request could not be decoded"}}
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
                {"id":"macos-handshake-1","protocol_version":2,"result":{"engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"1","capabilities":[]}}
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
            XCTFail("Expected the handshake EOF to be reported")
        } catch let error as EngineSupervisor.SupervisorError {
            XCTAssertEqual(error, .responseEndedBeforeHandshake)
        }

        let termination = try await supervisor.waitForTermination()
        let state = await supervisor.state
        XCTAssertEqual(termination, .init(reason: .exit, status: 17))
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
            printf '%s\\n' '{"id":"macos-handshake-1","protocol_version":1,"result":{"engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"1","capabilities":[]}}'
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
            printf '%s\\n' '{"id":"macos-handshake-1","protocol_version":1,"result":{"engine_version":"0.1.0","protocol_version":1,"compatible_client_version":"1","capabilities":[]}}'
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

    private func makeSupervisor(executableURL: URL) -> EngineSupervisor {
        EngineSupervisor(
            executableURL: executableURL,
            handshakeTimeout: .seconds(1),
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
}
