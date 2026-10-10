import Foundation
import XCTest
@testable import Souvenirs

final class EngineSupervisorTests: XCTestCase {
    private var fixtureDirectories: [URL] = []

    override func tearDownWithError() throws {
        for directory in fixtureDirectories {
            try? FileManager.default.removeItem(at: directory)
        }
        fixtureDirectories.removeAll()
        try super.tearDownWithError()
    }

    func testStartReportsRunningProcess() async throws {
        let executable = try makeExecutable(
            """
            trap 'exit 0' TERM
            while true; do sleep 0.05; done
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)

        let processIdentifier = try await supervisor.start()
        let state = await supervisor.state

        XCTAssertGreaterThan(processIdentifier, 0)
        XCTAssertEqual(state, .running(processIdentifier: processIdentifier))

        try await supervisor.stop()
    }

    func testStopTerminatesProcessAndReturnsToIdle() async throws {
        let executable = try makeExecutable(
            """
            trap 'exit 0' TERM
            while true; do sleep 0.05; done
            """
        )
        let supervisor = EngineSupervisor(executableURL: executable)
        _ = try await supervisor.start()

        try await supervisor.stop()

        let state = await supervisor.state
        let termination = await supervisor.lastTermination
        XCTAssertEqual(state, .idle)
        XCTAssertNotNil(termination)
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
            trap 'exit 0' TERM
            while true; do sleep 0.05; done
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

    func testMissingExecutableReturnsExplicitError() async {
        let missingExecutable = FileManager.default.temporaryDirectory
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

    private func makeExecutable(_ body: String) throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("EngineSupervisorTests-\(UUID().uuidString)")
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
