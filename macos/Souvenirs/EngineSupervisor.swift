import Darwin
import Foundation

public actor EngineSupervisor {
    public static let protocolVersion: UInt32 = 1

    public struct Handshake: Equatable, Sendable {
        public let engineVersion: String
        public let protocolVersion: UInt32
        public let compatibleClientVersion: String
        public let capabilities: [String]

        public init(
            engineVersion: String,
            protocolVersion: UInt32,
            compatibleClientVersion: String,
            capabilities: [String]
        ) {
            self.engineVersion = engineVersion
            self.protocolVersion = protocolVersion
            self.compatibleClientVersion = compatibleClientVersion
            self.capabilities = capabilities
        }
    }

    public struct Termination: Equatable, Sendable {
        public enum Reason: Equatable, Sendable {
            case exit
            case uncaughtSignal
        }

        public let reason: Reason
        public let status: Int32

        public init(reason: Reason, status: Int32) {
            self.reason = reason
            self.status = status
        }
    }

    public enum State: Equatable, Sendable {
        case idle
        case starting
        case running(processIdentifier: Int32)
        case stopping(processIdentifier: Int32)
        case exitedUnexpectedly(Termination)
    }

    public enum SupervisorError: Error, Equatable, LocalizedError, Sendable {
        case invalidExecutableURL(URL)
        case executableNotFound(URL)
        case executableNotExecutable(URL)
        case alreadyStarted(State)
        case notRunning
        case handshakeAlreadyRequested
        case launchFailed(String)
        case requestWriteFailed(String)
        case responseEndedBeforeHandshake
        case handshakeTimedOut
        case malformedResponse(String)
        case unexpectedResponseIdentifier(expected: String, actual: String?)
        case incompatibleProtocol(expected: UInt32, actual: UInt32)
        case protocolError(code: String, message: String)
        case inputCloseFailed(String)
        case shutdownTimedOut
        case forceKillFailed(Int32)
        case forcedTerminationTimedOut

        public var errorDescription: String? {
            switch self {
            case .invalidExecutableURL(let url):
                return "The engine executable URL is not a file URL: \(url.absoluteString)"
            case .executableNotFound(let url):
                return "The engine executable does not exist at \(url.path)"
            case .executableNotExecutable(let url):
                return "The engine file is not executable at \(url.path)"
            case .alreadyStarted(let state):
                return "The engine cannot start while its supervisor is in state \(state)"
            case .notRunning:
                return "The engine is not running"
            case .handshakeAlreadyRequested:
                return "The engine handshake has already been requested"
            case .launchFailed(let description):
                return "The engine failed to launch: \(description)"
            case .requestWriteFailed(let description):
                return "The engine handshake request could not be written: \(description)"
            case .responseEndedBeforeHandshake:
                return "The engine closed standard output before returning a handshake response"
            case .handshakeTimedOut:
                return "The engine did not return a handshake response before the timeout"
            case .malformedResponse(let description):
                return "The engine returned a malformed handshake response: \(description)"
            case .unexpectedResponseIdentifier(let expected, let actual):
                return "The engine returned response ID \(actual ?? "null"); expected \(expected)"
            case .incompatibleProtocol(let expected, let actual):
                return "The engine returned protocol version \(actual); expected \(expected)"
            case .protocolError(let code, let message):
                return "The engine returned protocol error \(code): \(message)"
            case .inputCloseFailed(let description):
                return "The engine standard input could not be closed: \(description)"
            case .shutdownTimedOut:
                return "The engine did not exit before the clean shutdown timeout"
            case .forceKillFailed(let errorNumber):
                return "The engine could not be forcibly terminated: errno \(errorNumber)"
            case .forcedTerminationTimedOut:
                return "The engine did not exit after it was forcibly terminated"
            }
        }
    }

    public private(set) var state: State = .idle
    public private(set) var lastTermination: Termination?

    private static let handshakeRequestIdentifier = "macos-handshake-1"

    private let executableURL: URL
    private let isProcessRunning: @Sendable (Process) -> Bool
    private let processTermination: @Sendable (Process) -> Termination
    private let handshakeTimeout: Duration
    private let shutdownTimeout: Duration
    private let terminationTimeout: Duration
    private var process: Process?
    private var standardInput: Pipe?
    private var standardOutput: Pipe?
    private var generation: UInt64 = 0
    private var stopRequested = false
    private var handshakeRequested = false
    private var terminationWaiters:
        [UUID: CheckedContinuation<Termination, any Error>] = [:]

    public init(executableURL: URL = EngineSupervisor.bundledExecutableURL()) {
        self.executableURL = executableURL
        self.isProcessRunning = { $0.isRunning }
        self.processTermination = { EngineSupervisor.termination(for: $0) }
        self.handshakeTimeout = .seconds(3)
        self.shutdownTimeout = .seconds(3)
        self.terminationTimeout = .seconds(1)
    }

    init(
        executableURL: URL,
        handshakeTimeout: Duration,
        shutdownTimeout: Duration,
        terminationTimeout: Duration
    ) {
        self.executableURL = executableURL
        self.isProcessRunning = { $0.isRunning }
        self.processTermination = { EngineSupervisor.termination(for: $0) }
        self.handshakeTimeout = handshakeTimeout
        self.shutdownTimeout = shutdownTimeout
        self.terminationTimeout = terminationTimeout
    }

    init(
        executableURL: URL,
        isProcessRunning: @escaping @Sendable (Process) -> Bool,
        processTermination: @escaping @Sendable (Process) -> Termination,
        handshakeTimeout: Duration = .seconds(3),
        shutdownTimeout: Duration = .seconds(3),
        terminationTimeout: Duration = .seconds(1)
    ) {
        self.executableURL = executableURL
        self.isProcessRunning = isProcessRunning
        self.processTermination = processTermination
        self.handshakeTimeout = handshakeTimeout
        self.shutdownTimeout = shutdownTimeout
        self.terminationTimeout = terminationTimeout
    }

    public static nonisolated func bundledExecutableURL(
        in bundle: Bundle = .main,
        executableName: String = "souvenirs-engine"
    ) -> URL {
        bundle.url(forAuxiliaryExecutable: executableName)
            ?? bundle.bundleURL
                .appendingPathComponent("Contents", isDirectory: true)
                .appendingPathComponent("MacOS", isDirectory: true)
                .appendingPathComponent(executableName, isDirectory: false)
    }

    @discardableResult
    public func start() throws -> Int32 {
        guard state == .idle || isUnexpectedExitState else {
            throw SupervisorError.alreadyStarted(state)
        }

        try validateExecutable()

        state = .starting
        lastTermination = nil
        stopRequested = false
        handshakeRequested = false
        generation &+= 1
        let activeGeneration = generation

        let launchedProcess = Process()
        let input = Pipe()
        let output = Pipe()
        launchedProcess.executableURL = executableURL
        launchedProcess.standardInput = input
        launchedProcess.standardOutput = output
        let processTermination = processTermination
        launchedProcess.terminationHandler = { [weak self] terminatedProcess in
            let termination = processTermination(terminatedProcess)
            Task {
                await self?.processDidTerminate(
                    termination,
                    generation: activeGeneration
                )
            }
        }

        do {
            try launchedProcess.run()
        } catch {
            state = .idle
            throw SupervisorError.launchFailed(error.localizedDescription)
        }

        process = launchedProcess
        standardInput = input
        standardOutput = output
        state = .running(processIdentifier: launchedProcess.processIdentifier)
        return launchedProcess.processIdentifier
    }

    public func handshake() async throws -> Handshake {
        guard process != nil,
              let input = standardInput,
              let output = standardOutput else {
            throw SupervisorError.notRunning
        }
        guard !handshakeRequested else {
            throw SupervisorError.handshakeAlreadyRequested
        }
        handshakeRequested = true

        let request = HandshakeRequest(
            id: Self.handshakeRequestIdentifier,
            protocolVersion: Self.protocolVersion,
            method: "handshake",
            params: EmptyParameters()
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]

        do {
            var data = try encoder.encode(request)
            data.append(0x0A)
            try input.fileHandleForWriting.write(contentsOf: data)
        } catch {
            throw SupervisorError.requestWriteFailed(error.localizedDescription)
        }

        let responseData: Data
        do {
            responseData = try await readHandshakeResponse(
                from: output.fileHandleForReading
            )
        } catch is TimeoutElapsed {
            throw SupervisorError.handshakeTimedOut
        }
        let response: HandshakeResponse
        do {
            response = try JSONDecoder().decode(HandshakeResponse.self, from: responseData)
        } catch {
            throw SupervisorError.malformedResponse(error.localizedDescription)
        }

        guard response.protocolVersion == Self.protocolVersion else {
            throw SupervisorError.incompatibleProtocol(
                expected: Self.protocolVersion,
                actual: response.protocolVersion
            )
        }
        if let error = response.error {
            throw SupervisorError.protocolError(code: error.code, message: error.message)
        }
        guard response.id == Self.handshakeRequestIdentifier else {
            throw SupervisorError.unexpectedResponseIdentifier(
                expected: Self.handshakeRequestIdentifier,
                actual: response.id
            )
        }
        guard let result = response.result else {
            throw SupervisorError.malformedResponse(
                "The response contains neither a result nor an error"
            )
        }
        guard result.protocolVersion == Self.protocolVersion else {
            throw SupervisorError.incompatibleProtocol(
                expected: Self.protocolVersion,
                actual: result.protocolVersion
            )
        }

        return Handshake(
            engineVersion: result.engineVersion,
            protocolVersion: result.protocolVersion,
            compatibleClientVersion: result.compatibleClientVersion,
            capabilities: result.capabilities
        )
    }

    public func stop() async throws {
        guard let activeProcess = process, let input = standardInput else {
            throw SupervisorError.notRunning
        }

        stopRequested = true
        state = .stopping(processIdentifier: activeProcess.processIdentifier)
        do {
            try input.fileHandleForWriting.close()
        } catch {
            stopRequested = false
            state = .running(processIdentifier: activeProcess.processIdentifier)
            throw SupervisorError.inputCloseFailed(error.localizedDescription)
        }

        observeTerminationIfNeeded(for: activeProcess)
        if try await didTerminate(within: shutdownTimeout) {
            return
        }

        if isProcessRunning(activeProcess) {
            activeProcess.terminate()
        } else {
            observeTerminationIfNeeded(for: activeProcess)
        }
        if try await didTerminate(within: terminationTimeout) {
            throw SupervisorError.shutdownTimedOut
        }

        if isProcessRunning(activeProcess) {
            if Darwin.kill(activeProcess.processIdentifier, SIGKILL) == -1 {
                let errorNumber = errno
                if errorNumber != ESRCH {
                    throw SupervisorError.forceKillFailed(errorNumber)
                }
            }
        } else {
            observeTerminationIfNeeded(for: activeProcess)
        }
        guard try await didTerminate(within: terminationTimeout) else {
            throw SupervisorError.forcedTerminationTimedOut
        }
        throw SupervisorError.shutdownTimedOut
    }

    public func waitForTermination() async throws -> Termination {
        if let lastTermination, process == nil {
            return lastTermination
        }

        guard process != nil else {
            throw SupervisorError.notRunning
        }

        return try await waitForActiveProcessTermination()
    }

    private var isUnexpectedExitState: Bool {
        if case .exitedUnexpectedly = state {
            return true
        }
        return false
    }

    private func validateExecutable() throws {
        guard executableURL.isFileURL else {
            throw SupervisorError.invalidExecutableURL(executableURL)
        }

        let fileManager = FileManager.default
        guard fileManager.fileExists(atPath: executableURL.path) else {
            throw SupervisorError.executableNotFound(executableURL)
        }
        guard fileManager.isExecutableFile(atPath: executableURL.path) else {
            throw SupervisorError.executableNotExecutable(executableURL)
        }
    }

    private nonisolated static func readResponseLine(
        from fileHandle: FileHandle
    ) async throws -> Data {
        try await ResponseLineReader().read(from: fileHandle)
    }

    private func readHandshakeResponse(from fileHandle: FileHandle) async throws -> Data {
        let timeout = handshakeTimeout
        return try await withThrowingTaskGroup(of: Data.self) { group in
            group.addTask {
                try await Self.readResponseLine(from: fileHandle)
            }
            group.addTask {
                try await Task.sleep(for: timeout)
                throw TimeoutElapsed()
            }
            group.addTask {
                _ = try await self.waitForActiveProcessTermination()
                throw SupervisorError.responseEndedBeforeHandshake
            }

            guard let response = try await group.next() else {
                throw CancellationError()
            }
            group.cancelAll()
            return response
        }
    }

    private func didTerminate(within timeout: Duration) async throws -> Bool {
        do {
            _ = try await Self.withTimeout(timeout) {
                try await self.waitForActiveProcessTermination()
            }
            return true
        } catch is TimeoutElapsed {
            return false
        }
    }

    private func waitForActiveProcessTermination() async throws -> Termination {
        if let lastTermination, process == nil {
            return lastTermination
        }

        let identifier = UUID()
        return try await withTaskCancellationHandler {
            try Task.checkCancellation()
            return try await withCheckedThrowingContinuation { continuation in
                terminationWaiters[identifier] = continuation
            }
        } onCancel: {
            Task {
                await self.cancelTerminationWaiter(identifier)
            }
        }
    }

    private func cancelTerminationWaiter(_ identifier: UUID) {
        terminationWaiters.removeValue(forKey: identifier)?
            .resume(throwing: CancellationError())
    }

    private func observeTerminationIfNeeded(for activeProcess: Process) {
        guard process != nil, !isProcessRunning(activeProcess) else {
            return
        }
        processDidTerminate(
            processTermination(activeProcess),
            generation: generation
        )
    }

    private func processDidTerminate(
        _ termination: Termination,
        generation terminatedGeneration: UInt64
    ) {
        guard terminatedGeneration == generation, process != nil else {
            return
        }

        let wasRequested = stopRequested
        process = nil
        standardInput = nil
        standardOutput = nil
        stopRequested = false
        handshakeRequested = false
        lastTermination = termination
        state = wasRequested ? .idle : .exitedUnexpectedly(termination)

        let waiters = terminationWaiters.values
        terminationWaiters.removeAll()
        for waiter in waiters {
            waiter.resume(returning: termination)
        }
    }

    private nonisolated static func termination(for process: Process) -> Termination {
        let reason: Termination.Reason
        switch process.terminationReason {
        case .exit:
            reason = .exit
        case .uncaughtSignal:
            reason = .uncaughtSignal
        @unknown default:
            reason = .uncaughtSignal
        }
        return Termination(reason: reason, status: process.terminationStatus)
    }

    private nonisolated static func withTimeout<Value: Sendable>(
        _ timeout: Duration,
        operation: @escaping @Sendable () async throws -> Value
    ) async throws -> Value {
        try await withThrowingTaskGroup(of: Value.self) { group in
            group.addTask {
                try await operation()
            }
            group.addTask {
                try await Task.sleep(for: timeout)
                throw TimeoutElapsed()
            }

            guard let result = try await group.next() else {
                throw CancellationError()
            }
            group.cancelAll()
            return result
        }
    }
}

private struct TimeoutElapsed: Error {}

private final class ResponseLineReader: @unchecked Sendable {
    private let lock = NSLock()
    private var buffer = Data()
    private var continuation: CheckedContinuation<Data, any Error>?
    private var result: Result<Data, any Error>?

    func read(from fileHandle: FileHandle) async throws -> Data {
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                beginReading(from: fileHandle, continuation: continuation)
            }
        } onCancel: {
            finish(.failure(CancellationError()), fileHandle: fileHandle)
        }
    }

    private func beginReading(
        from fileHandle: FileHandle,
        continuation newContinuation: CheckedContinuation<Data, any Error>
    ) {
        lock.lock()
        if let result {
            lock.unlock()
            newContinuation.resume(with: result)
            return
        }

        continuation = newContinuation
        fileHandle.readabilityHandler = { [weak self] readableHandle in
            self?.consumeAvailableData(from: readableHandle)
        }
        lock.unlock()
    }

    private func consumeAvailableData(from fileHandle: FileHandle) {
        let data = fileHandle.availableData
        guard !data.isEmpty else {
            finish(
                .failure(EngineSupervisor.SupervisorError.responseEndedBeforeHandshake),
                fileHandle: fileHandle
            )
            return
        }

        lock.lock()
        guard result == nil else {
            lock.unlock()
            return
        }
        buffer.append(data)
        guard let newlineIndex = buffer.firstIndex(of: 0x0A) else {
            lock.unlock()
            return
        }
        let line = Data(buffer[..<newlineIndex])
        lock.unlock()
        finish(.success(line), fileHandle: fileHandle)
    }

    private func finish(
        _ newResult: Result<Data, any Error>,
        fileHandle: FileHandle
    ) {
        lock.lock()
        guard result == nil else {
            lock.unlock()
            return
        }

        result = newResult
        let pendingContinuation = continuation
        continuation = nil
        fileHandle.readabilityHandler = nil
        lock.unlock()
        pendingContinuation?.resume(with: newResult)
    }
}

private struct EmptyParameters: Encodable {}

private struct HandshakeRequest: Encodable {
    let id: String
    let protocolVersion: UInt32
    let method: String
    let params: EmptyParameters

    enum CodingKeys: String, CodingKey {
        case id
        case protocolVersion = "protocol_version"
        case method
        case params
    }
}

private struct HandshakeResponse: Decodable {
    let id: String?
    let protocolVersion: UInt32
    let result: HandshakeResult?
    let error: ProtocolErrorResponse?

    enum CodingKeys: String, CodingKey {
        case id
        case protocolVersion = "protocol_version"
        case result
        case error
    }
}

private struct HandshakeResult: Decodable {
    let engineVersion: String
    let protocolVersion: UInt32
    let compatibleClientVersion: String
    let capabilities: [String]

    enum CodingKeys: String, CodingKey {
        case engineVersion = "engine_version"
        case protocolVersion = "protocol_version"
        case compatibleClientVersion = "compatible_client_version"
        case capabilities
    }
}

private struct ProtocolErrorResponse: Decodable {
    let code: String
    let message: String
}
