import Foundation

public actor EngineSupervisor {
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
        case launchFailed(String)

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
            case .launchFailed(let description):
                return "The engine failed to launch: \(description)"
            }
        }
    }

    public private(set) var state: State = .idle
    public private(set) var lastTermination: Termination?

    private let executableURL: URL
    private let isProcessRunning: @Sendable (Process) -> Bool
    private let processTermination: @Sendable (Process) -> Termination
    private var process: Process?
    private var standardInput: Pipe?
    private var generation: UInt64 = 0
    private var stopRequested = false
    private var terminationWaiters: [CheckedContinuation<Termination, Never>] = []

    public init(executableURL: URL = EngineSupervisor.bundledExecutableURL()) {
        self.executableURL = executableURL
        self.isProcessRunning = { $0.isRunning }
        self.processTermination = { EngineSupervisor.termination(for: $0) }
    }

    init(
        executableURL: URL,
        isProcessRunning: @escaping @Sendable (Process) -> Bool,
        processTermination: @escaping @Sendable (Process) -> Termination
    ) {
        self.executableURL = executableURL
        self.isProcessRunning = isProcessRunning
        self.processTermination = processTermination
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
        generation &+= 1
        let activeGeneration = generation

        let launchedProcess = Process()
        let input = Pipe()
        launchedProcess.executableURL = executableURL
        launchedProcess.standardInput = input
        launchedProcess.terminationHandler = { [weak self] terminatedProcess in
            let reason: Termination.Reason
            switch terminatedProcess.terminationReason {
            case .exit:
                reason = .exit
            case .uncaughtSignal:
                reason = .uncaughtSignal
            @unknown default:
                reason = .uncaughtSignal
            }

            let termination = Termination(
                reason: reason,
                status: terminatedProcess.terminationStatus
            )
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
        state = .running(processIdentifier: launchedProcess.processIdentifier)
        return launchedProcess.processIdentifier
    }

    public func stop() async throws {
        guard let activeProcess = process else {
            throw SupervisorError.notRunning
        }

        stopRequested = true
        state = .stopping(processIdentifier: activeProcess.processIdentifier)
        try? standardInput?.fileHandleForWriting.close()

        if isProcessRunning(activeProcess) {
            activeProcess.terminate()
        } else {
            processDidTerminate(
                processTermination(activeProcess),
                generation: generation
            )
        }

        _ = await waitForActiveProcessTermination()
    }

    public func waitForTermination() async throws -> Termination {
        if let lastTermination, process == nil {
            return lastTermination
        }

        guard process != nil else {
            throw SupervisorError.notRunning
        }

        return await waitForActiveProcessTermination()
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

    private func waitForActiveProcessTermination() async -> Termination {
        if let lastTermination, process == nil {
            return lastTermination
        }

        return await withCheckedContinuation { continuation in
            terminationWaiters.append(continuation)
        }
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
        stopRequested = false
        lastTermination = termination
        state = wasRequested ? .idle : .exitedUnexpectedly(termination)

        let waiters = terminationWaiters
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
}
