import Darwin
import Foundation

public actor EngineSupervisor {
    public static let protocolVersion: UInt32 = 1
    public static let protocolClientVersion = "0.1.0"

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

    public struct CancellationAccepted: Equatable, Sendable {
        public let operationID: String
    }

    public struct CatalogLocation: Codable, Equatable, Sendable {
        public let path: String
        public let volumeName: String
        public let displayName: String?
        public let detail: String
    }

    public enum ImportDestinationMode: String, Codable, Equatable, Sendable {
        case create
        case existing
    }

    public enum ImportDateResolution: String, Codable, Equatable, Sendable {
        case asShown = "as_shown"
        case convertFromUTC = "convert_from_utc"
    }

    public struct ImportScan: Codable, Equatable, Sendable {
        public let sourcePath: String
        public let sourceName: String
        public let fileCount: Int
        public let mediaCount: Int
        public let photoCount: Int
        public let videoCount: Int
        public let unsupportedCount: Int
        public let uncertainTimeCount: Int
        public let totalBytes: UInt64
        public let suggestedCollectionName: String
        public let suggestedAcceptedDate: String

        enum CodingKeys: String, CodingKey {
            case sourcePath = "source_path"
            case sourceName = "source_name"
            case fileCount = "file_count"
            case mediaCount = "media_count"
            case photoCount = "photo_count"
            case videoCount = "video_count"
            case unsupportedCount = "unsupported_count"
            case uncertainTimeCount = "uncertain_time_count"
            case totalBytes = "total_bytes"
            case suggestedCollectionName = "suggested_collection_name"
            case suggestedAcceptedDate = "suggested_accepted_date"
        }
    }

    public struct ImportReview: Codable, Equatable, Sendable {
        public let scan: ImportScan
        public let collectionName: String
        public let destinationMode: ImportDestinationMode
        public let destinationPath: String
        public let acceptedDate: String
        public let dateResolution: ImportDateResolution?
        public let requiredDecisionCount: Int
        public let transferCount: Int
        public let skippedCount: Int
        public let leftOnSourceCount: Int
        public let operationCount: Int
        public let requiredSpaceBytes: UInt64
        public let canStart: Bool

        enum CodingKeys: String, CodingKey {
            case scan
            case collectionName = "collection_name"
            case destinationMode = "destination_mode"
            case destinationPath = "destination_path"
            case acceptedDate = "accepted_date"
            case dateResolution = "date_resolution"
            case requiredDecisionCount = "required_decision_count"
            case transferCount = "transfer_count"
            case skippedCount = "skipped_count"
            case leftOnSourceCount = "left_on_source_count"
            case operationCount = "operation_count"
            case requiredSpaceBytes = "required_space_bytes"
            case canStart = "can_start"
        }
    }

    public struct UnfinishedOperation: Codable, Equatable, Sendable {
        public let operationID: String
        public let collectionName: String
        public let sourceName: String
        public let copiedCount: Int
        public let totalCount: Int
        public let sourceAvailable: Bool

        enum CodingKeys: String, CodingKey {
            case operationID = "operation_id"
            case collectionName = "collection_name"
            case sourceName = "source_name"
            case copiedCount = "copied_count"
            case totalCount = "total_count"
            case sourceAvailable = "source_available"
        }
    }

    public struct OperationEvent: Equatable, Sendable {
        public enum Payload: Equatable, Sendable {
            case started(name: String)
            case progress(
                completedUnits: UInt64,
                totalUnits: UInt64?,
                message: String?
            )
            case completed(result: Data)
            case failed(code: String, message: String)
            case cancelled
        }

        public let operationID: String
        public let sequence: UInt64
        public let payload: Payload

        public var isTerminal: Bool {
            switch payload {
            case .completed, .failed, .cancelled:
                return true
            case .started, .progress:
                return false
            }
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
        case responseStreamEnded
        case responseEndedBeforeHandshake
        case requestTimedOut(String)
        case handshakeTimedOut
        case malformedResponse(String)
        case unexpectedResponseIdentifier(expected: String, actual: String?)
        case incompatibleProtocol(expected: UInt32, actual: UInt32)
        case incompatibleClientVersion(expected: String, actual: String)
        case protocolError(code: String, message: String)
        case operationStreamAlreadyRequested(String)
        case outOfOrderEvent(operationID: String, expected: UInt64, actual: UInt64)
        case unexpectedTermination(Termination)
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
                return "The engine request could not be written: \(description)"
            case .responseStreamEnded:
                return "The engine closed its response stream"
            case .responseEndedBeforeHandshake:
                return "The engine closed standard output before returning a handshake response"
            case .requestTimedOut(let identifier):
                return "The engine did not respond to request \(identifier) before the timeout"
            case .handshakeTimedOut:
                return "The engine did not return a handshake response before the timeout"
            case .malformedResponse(let description):
                return "The engine returned a malformed response: \(description)"
            case .unexpectedResponseIdentifier(let expected, let actual):
                return "The engine returned response ID \(actual ?? "null"); expected \(expected)"
            case .incompatibleProtocol(let expected, let actual):
                return "The engine returned protocol version \(actual); expected \(expected)"
            case .incompatibleClientVersion(let expected, let actual):
                return "The engine supports client version \(actual); expected \(expected)"
            case .protocolError(let code, let message):
                return "The engine returned protocol error \(code): \(message)"
            case .operationStreamAlreadyRequested(let operationID):
                return "Operation \(operationID) already has an event stream"
            case .outOfOrderEvent(let operationID, let expected, let actual):
                return "Operation \(operationID) returned event \(actual); expected \(expected)"
            case .unexpectedTermination(let termination):
                return "The engine stopped unexpectedly with status \(termination.status)"
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

    private struct PendingResponse {
        let continuation: CheckedContinuation<Data, any Error>
        let timeoutTask: Task<Void, Never>
    }

    private struct OperationSubscription {
        let token: UUID
        let continuation: AsyncThrowingStream<OperationEvent, any Error>.Continuation
    }

    private struct ObservedTermination {
        let termination: Termination
        let wasRequested: Bool
    }

    private let executableURL: URL
    private let arguments: [String]
    private let isProcessRunning: @Sendable (Process) -> Bool
    private let processTermination: @Sendable (Process) -> Termination
    private let handshakeTimeout: Duration
    private let requestTimeout: Duration
    private let shutdownTimeout: Duration
    private let terminationTimeout: Duration
    private let responseEOFGrace: Duration
    private var process: Process?
    private var standardInput: Pipe?
    private var standardOutput: Pipe?
    private var writer: ProtocolWriter?
    private var lineReader: ProtocolLineReader?
    private var generation: UInt64 = 0
    private var requestSequence: UInt64 = 0
    private var stopRequested = false
    private var handshakeRequested = false
    private var pendingResponses: [String: PendingResponse] = [:]
    private var operationSubscriptions: [String: OperationSubscription] = [:]
    private var bufferedOperationEvents: [String: [OperationEvent]] = [:]
    private var expectedEventSequences: [String: UInt64] = [:]
    private var knownOperations: Set<String> = []
    private var terminalOperations: Set<String> = []
    private var operationFailures: [String: SupervisorError] = [:]
    private var pendingReadBatches: [UInt64: ([Data], Bool)] = [:]
    private var nextReadBatch: UInt64 = 0
    private var responseStreamHasEnded = false
    private var protocolConnectionError: SupervisorError?
    private var observedTermination: ObservedTermination?
    private var responseEOFTask: Task<Void, Never>?
    private var terminationDrainTask: Task<Void, Never>?
    private var terminationWaiters:
        [UUID: CheckedContinuation<Termination, any Error>] = [:]

    public init(
        executableURL: URL = EngineSupervisor.bundledExecutableURL(),
        arguments: [String] = []
    ) {
        self.executableURL = executableURL
        self.arguments = arguments
        self.isProcessRunning = { $0.isRunning }
        self.processTermination = { EngineSupervisor.termination(for: $0) }
        self.handshakeTimeout = .seconds(3)
        self.requestTimeout = .seconds(30)
        self.shutdownTimeout = .seconds(3)
        self.terminationTimeout = .seconds(1)
        self.responseEOFGrace = .seconds(1)
    }

    init(
        executableURL: URL,
        handshakeTimeout: Duration,
        requestTimeout: Duration = .seconds(3),
        shutdownTimeout: Duration,
        terminationTimeout: Duration
    ) {
        self.executableURL = executableURL
        self.arguments = []
        self.isProcessRunning = { $0.isRunning }
        self.processTermination = { EngineSupervisor.termination(for: $0) }
        self.handshakeTimeout = handshakeTimeout
        self.requestTimeout = requestTimeout
        self.shutdownTimeout = shutdownTimeout
        self.terminationTimeout = terminationTimeout
        self.responseEOFGrace = terminationTimeout
    }

    init(
        executableURL: URL,
        isProcessRunning: @escaping @Sendable (Process) -> Bool,
        processTermination: @escaping @Sendable (Process) -> Termination,
        handshakeTimeout: Duration = .seconds(3),
        requestTimeout: Duration = .seconds(3),
        shutdownTimeout: Duration = .seconds(3),
        terminationTimeout: Duration = .seconds(1)
    ) {
        self.executableURL = executableURL
        self.arguments = []
        self.isProcessRunning = isProcessRunning
        self.processTermination = processTermination
        self.handshakeTimeout = handshakeTimeout
        self.requestTimeout = requestTimeout
        self.shutdownTimeout = shutdownTimeout
        self.terminationTimeout = terminationTimeout
        self.responseEOFGrace = terminationTimeout
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
        requestSequence = 0
        resetProtocolState()
        generation &+= 1
        let activeGeneration = generation

        let launchedProcess = Process()
        let input = Pipe()
        let output = Pipe()
        launchedProcess.executableURL = executableURL
        launchedProcess.arguments = arguments
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

        let reader = ProtocolLineReader()
        reader.start(
            fileHandle: output.fileHandleForReading,
            onBatch: { [weak self] sequence, lines, ended in
                Task {
                    await self?.receiveBatch(
                        sequence: sequence,
                        lines: lines,
                        ended: ended,
                        generation: activeGeneration
                    )
                }
            }
        )

        process = launchedProcess
        standardInput = input
        standardOutput = output
        writer = ProtocolWriter(fileHandle: input.fileHandleForWriting)
        lineReader = reader
        state = .running(processIdentifier: launchedProcess.processIdentifier)
        return launchedProcess.processIdentifier
    }

    public func handshake() async throws -> Handshake {
        guard !handshakeRequested else {
            throw SupervisorError.handshakeAlreadyRequested
        }
        handshakeRequested = true

        let result: HandshakeResult
        do {
            result = try await sendRequest(
                identifier: Self.handshakeRequestIdentifier,
                method: "handshake",
                parameters: HandshakeParameters(
                    clientVersion: Self.protocolClientVersion
                ),
                timeout: handshakeTimeout
            )
        } catch SupervisorError.requestTimedOut {
            throw SupervisorError.handshakeTimedOut
        } catch SupervisorError.responseStreamEnded {
            throw SupervisorError.responseEndedBeforeHandshake
        }

        guard result.type == "handshake" else {
            throw SupervisorError.malformedResponse(
                "The response result type is not handshake"
            )
        }
        guard result.protocolVersion == Self.protocolVersion else {
            throw SupervisorError.incompatibleProtocol(
                expected: Self.protocolVersion,
                actual: result.protocolVersion
            )
        }
        guard result.compatibleClientVersion == Self.protocolClientVersion else {
            throw SupervisorError.incompatibleClientVersion(
                expected: Self.protocolClientVersion,
                actual: result.compatibleClientVersion
            )
        }

        return Handshake(
            engineVersion: result.engineVersion,
            protocolVersion: result.protocolVersion,
            compatibleClientVersion: result.compatibleClientVersion,
            capabilities: result.capabilities
        )
    }

    public func requestOperation<Parameters: Encodable & Sendable>(
        method: String,
        parameters: Parameters
    ) async throws -> String {
        let timeout: Duration = method == "operation.start"
            ? .seconds(600)
            : requestTimeout
        let result: OperationAcceptedResult = try await sendRequest(
            method: method,
            parameters: parameters,
            timeout: timeout
        )
        guard result.type == "operation_accepted" else {
            throw SupervisorError.malformedResponse(
                "The response result type is not operation_accepted"
            )
        }
        knownOperations.insert(result.operationID)
        return result.operationID
    }

    public func cancel(operationID: String) async throws -> CancellationAccepted {
        let result: CancellationAcceptedResult = try await sendRequest(
            method: "operation.cancel",
            parameters: CancelOperationParameters(operationID: operationID),
            timeout: requestTimeout
        )
        guard result.type == "cancellation_accepted" else {
            throw SupervisorError.malformedResponse(
                "The response result type is not cancellation_accepted"
            )
        }
        guard result.operationID == operationID else {
            throw SupervisorError.malformedResponse(
                "The cancellation response returned a different operation_id"
            )
        }
        return CancellationAccepted(operationID: result.operationID)
    }

    public func validateCatalog(path: String, intent: String) async throws -> CatalogLocation {
        let result: CatalogResult = try await sendRequest(
            method: "catalog.validate",
            parameters: CatalogValidationParameters(path: path, intent: intent),
            timeout: requestTimeout
        )
        return try catalogLocation(from: result)
    }

    public func createCatalog(path: String, displayName: String) async throws -> CatalogLocation {
        let result: CatalogResult = try await sendRequest(
            method: "catalog.create",
            parameters: CreateCatalogParameters(path: path, displayName: displayName),
            timeout: requestTimeout
        )
        return try catalogLocation(from: result)
    }

    public func openCatalog(path: String) async throws -> CatalogLocation {
        let result: CatalogResult = try await sendRequest(
            method: "catalog.open",
            parameters: CatalogPathParameters(path: path),
            timeout: requestTimeout
        )
        return try catalogLocation(from: result)
    }

    public func scanImport(sourcePath: String) async throws -> ImportScan {
        let result: ImportScanResult = try await sendRequest(
            method: "import.scan",
            parameters: ImportScanParameters(sourcePath: sourcePath),
            timeout: requestTimeout
        )
        guard result.type == "import_scan" else {
            throw SupervisorError.malformedResponse(
                "The response result type is not import_scan"
            )
        }
        return result.scan
    }

    public func reviewImport(
        catalogPath: String,
        sourcePath: String,
        collectionName: String,
        destinationMode: ImportDestinationMode,
        acceptedDate: String,
        dateResolution: ImportDateResolution?
    ) async throws -> ImportReview {
        let result: ImportReviewResult = try await sendRequest(
            method: "import.review",
            parameters: ImportReviewParameters(
                catalogPath: catalogPath,
                sourcePath: sourcePath,
                collectionName: collectionName,
                destinationMode: destinationMode,
                acceptedDate: acceptedDate,
                dateResolution: dateResolution
            ),
            timeout: requestTimeout
        )
        guard result.type == "import_review" else {
            throw SupervisorError.malformedResponse(
                "The response result type is not import_review"
            )
        }
        return result.review
    }

    public func inspectUnfinishedOperation(
        catalogPath: String
    ) async throws -> UnfinishedOperation? {
        let result: OperationInspectionResult = try await sendRequest(
            method: "operation.inspect",
            parameters: CatalogPathParameters(path: catalogPath),
            timeout: requestTimeout
        )
        guard result.type == "operation_inspection" else {
            throw SupervisorError.malformedResponse(
                "The response result type is not operation_inspection"
            )
        }
        return result.operation
    }

    private func catalogLocation(from result: CatalogResult) throws -> CatalogLocation {
        guard result.type == "catalog" else {
            throw SupervisorError.malformedResponse(
                "The response result type is not catalog"
            )
        }
        return CatalogLocation(
            path: result.path,
            volumeName: result.volumeName,
            displayName: result.displayName,
            detail: result.detail
        )
    }

    public func events(
        for operationID: String
    ) throws -> AsyncThrowingStream<OperationEvent, any Error> {
        guard operationSubscriptions[operationID] == nil else {
            throw SupervisorError.operationStreamAlreadyRequested(operationID)
        }

        let buffered = bufferedOperationEvents.removeValue(forKey: operationID) ?? []
        let terminal = terminalOperations.contains(operationID)
        let failure = operationFailures.removeValue(forKey: operationID)
            ?? (terminal ? nil : protocolConnectionError)
        let token = UUID()
        return AsyncThrowingStream { continuation in
            operationSubscriptions[operationID] = OperationSubscription(
                token: token,
                continuation: continuation
            )
            continuation.onTermination = { [weak self] _ in
                Task {
                    await self?.removeOperationSubscription(
                        operationID,
                        token: token
                    )
                }
            }
            for event in buffered {
                continuation.yield(event)
            }
            if let failure {
                continuation.finish(throwing: failure)
                operationSubscriptions.removeValue(forKey: operationID)
                pruneOperation(operationID)
            } else if terminal {
                continuation.finish()
                operationSubscriptions.removeValue(forKey: operationID)
                pruneOperation(operationID)
            }
        }
    }

    public func stop() async throws {
        guard let activeProcess = process, let writer else {
            throw SupervisorError.notRunning
        }

        stopRequested = true
        state = .stopping(processIdentifier: activeProcess.processIdentifier)
        do {
            try await writer.close()
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

    private func sendRequest<Parameters, Result>(
        identifier: String? = nil,
        method: String,
        parameters: Parameters,
        timeout: Duration
    ) async throws -> Result
    where Parameters: Encodable & Sendable, Result: Decodable & Sendable {
        guard process != nil, case .running = state else {
            throw SupervisorError.notRunning
        }
        if let protocolConnectionError {
            throw protocolConnectionError
        }
        guard let writer else {
            throw SupervisorError.notRunning
        }

        let requestID = identifier ?? nextRequestIdentifier()
        let request = RequestEnvelope(
            id: requestID,
            protocolVersion: Self.protocolVersion,
            method: method,
            params: parameters
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        var data: Data
        do {
            data = try encoder.encode(request)
            data.append(0x0A)
        } catch {
            throw SupervisorError.requestWriteFailed(String(describing: error))
        }

        let responseData: Data = try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                let timeoutTask = Task { [weak self] in
                    try? await Task.sleep(for: timeout)
                    guard !Task.isCancelled else {
                        return
                    }
                    await self?.timeOutRequest(requestID)
                }
                pendingResponses[requestID] = PendingResponse(
                    continuation: continuation,
                    timeoutTask: timeoutTask
                )
                writer.write(data) { [weak self] error in
                    guard let error else {
                        return
                    }
                    Task {
                        await self?.finishRequest(
                            requestID,
                            with: .failure(
                                SupervisorError.requestWriteFailed(
                                    error.localizedDescription
                                )
                            )
                        )
                    }
                }
            }
        } onCancel: {
            Task {
                await self.cancelRequest(requestID)
            }
        }

        do {
            let response = try JSONDecoder().decode(
                SuccessResponse<Result>.self,
                from: responseData
            )
            guard response.kind == "response", response.status == "success" else {
                throw SupervisorError.malformedResponse(
                    "The request did not return a success response"
                )
            }
            guard response.id == requestID else {
                throw SupervisorError.unexpectedResponseIdentifier(
                    expected: requestID,
                    actual: response.id
                )
            }
            return response.result
        } catch let error as SupervisorError {
            throw error
        } catch {
            throw SupervisorError.malformedResponse(String(describing: error))
        }
    }

    private func receiveBatch(
        sequence: UInt64,
        lines: [Data],
        ended: Bool,
        generation receivedGeneration: UInt64
    ) {
        guard receivedGeneration == generation, process != nil else {
            return
        }
        pendingReadBatches[sequence] = (lines, ended)
        while let batch = pendingReadBatches.removeValue(forKey: nextReadBatch) {
            nextReadBatch &+= 1
            for line in batch.0 {
                receive(line)
            }
            if batch.1 {
                responseStreamDidEnd(generation: receivedGeneration)
            }
        }
    }

    private func receive(_ line: Data) {
        let object: [String: Any]
        do {
            guard let decoded = try JSONSerialization.jsonObject(with: line)
                as? [String: Any] else {
                throw SupervisorError.malformedResponse(
                    "The protocol message is not a JSON object"
                )
            }
            object = decoded
        } catch let error as SupervisorError {
            failUncorrelatedMessage(error)
            return
        } catch {
            failUncorrelatedMessage(
                .malformedResponse(String(describing: error))
            )
            return
        }

        guard let versionNumber = object["protocol_version"] as? NSNumber else {
            failUncorrelatedMessage(
                .malformedResponse("The protocol message has no protocol_version")
            )
            return
        }
        let actualVersion = versionNumber.uint32Value
        let identifier = object["id"] as? String
        let kind = object["kind"] as? String
        guard actualVersion == Self.protocolVersion else {
            let error = SupervisorError.incompatibleProtocol(
                expected: Self.protocolVersion,
                actual: actualVersion
            )
            if kind == "event" {
                failOperationMessage(line, error: error)
            } else {
                finishCorrelatedOrOnlyRequest(
                    identifier: identifier,
                    error: error
                )
            }
            return
        }

        switch kind {
        case "response":
            receiveResponse(line, object: object, identifier: identifier)
        case "event":
            receiveEvent(line)
        default:
            failUncorrelatedMessage(
                .malformedResponse("The protocol message kind is unsupported")
            )
        }
    }

    private func receiveResponse(
        _ line: Data,
        object: [String: Any],
        identifier: String?
    ) {
        let status = object["status"] as? String
        if status == "error" {
            do {
                let response = try JSONDecoder().decode(
                    ErrorResponse.self,
                    from: line
                )
                finishCorrelatedOrOnlyRequest(
                    identifier: identifier,
                    error: .protocolError(
                        code: response.error.code,
                        message: response.error.message
                    )
                )
            } catch {
                finishCorrelatedOrOnlyRequest(
                    identifier: identifier,
                    error: .malformedResponse(String(describing: error))
                )
            }
            return
        }

        guard status == "success" else {
            finishCorrelatedOrOnlyRequest(
                identifier: identifier,
                error: .malformedResponse(
                    "The response status is unsupported"
                )
            )
            return
        }
        finishCorrelatedOrOnlyRequest(identifier: identifier, data: line)
    }

    private func receiveEvent(_ line: Data) {
        do {
            let event = try decodeOperationEvent(line)
            route(event)
        } catch let error as SupervisorError {
            failOperationMessage(line, error: error)
        } catch {
            failOperationMessage(
                line,
                error: .malformedResponse(String(describing: error))
            )
        }
    }

    private func decodeOperationEvent(_ line: Data) throws -> OperationEvent {
        let envelope = try JSONDecoder().decode(OperationEventEnvelope.self, from: line)
        guard envelope.kind == "event" else {
            throw SupervisorError.malformedResponse(
                "The operation message kind is not event"
            )
        }

        let payload: OperationEvent.Payload
        switch envelope.event {
        case "started":
            guard let name = envelope.name else {
                throw SupervisorError.malformedResponse(
                    "A started event requires a name"
                )
            }
            payload = .started(name: name)
        case "progress":
            guard let completedUnits = envelope.completedUnits else {
                throw SupervisorError.malformedResponse(
                    "A progress event requires completed_units"
                )
            }
            payload = .progress(
                completedUnits: completedUnits,
                totalUnits: envelope.totalUnits,
                message: envelope.message
            )
        case "completed":
            let result = try completedResult(from: line)
            payload = .completed(result: result)
        case "failed":
            guard let error = envelope.error else {
                throw SupervisorError.malformedResponse(
                    "A failed event requires an error"
                )
            }
            payload = .failed(code: error.code, message: error.message)
        case "cancelled":
            payload = .cancelled
        default:
            throw SupervisorError.malformedResponse(
                "The operation event type is unsupported"
            )
        }

        return OperationEvent(
            operationID: envelope.operationID,
            sequence: envelope.sequence,
            payload: payload
        )
    }

    private func completedResult(from line: Data) throws -> Data {
        guard let object = try JSONSerialization.jsonObject(with: line)
            as? [String: Any],
            let result = object["result"] else {
            throw SupervisorError.malformedResponse(
                "A completed event requires a result"
            )
        }
        return try JSONSerialization.data(
            withJSONObject: result,
            options: [.fragmentsAllowed, .sortedKeys]
        )
    }

    private func route(_ event: OperationEvent) {
        let expected = expectedEventSequences[event.operationID] ?? 0
        guard event.sequence == expected else {
            let error = SupervisorError.outOfOrderEvent(
                operationID: event.operationID,
                expected: expected,
                actual: event.sequence
            )
            let subscription = operationSubscriptions.removeValue(
                forKey: event.operationID
            )
            subscription?.continuation.finish(throwing: error)
            bufferedOperationEvents.removeValue(forKey: event.operationID)
            operationFailures[event.operationID] = error
            terminalOperations.insert(event.operationID)
            if subscription != nil {
                pruneOperation(event.operationID)
            }
            return
        }

        expectedEventSequences[event.operationID] = expected + 1
        if let subscription = operationSubscriptions[event.operationID] {
            subscription.continuation.yield(event)
            if event.isTerminal {
                subscription.continuation.finish()
                operationSubscriptions.removeValue(forKey: event.operationID)
                terminalOperations.insert(event.operationID)
                pruneOperation(event.operationID)
            }
        } else {
            bufferedOperationEvents[event.operationID, default: []].append(event)
            if event.isTerminal {
                terminalOperations.insert(event.operationID)
            }
        }
    }

    private func finishCorrelatedOrOnlyRequest(
        identifier: String?,
        data: Data
    ) {
        if let identifier, pendingResponses[identifier] != nil {
            finishRequest(identifier, with: .success(data))
            return
        }
        if identifier != nil {
            return
        }
        guard pendingResponses.count == 1, let expected = pendingResponses.keys.first else {
            return
        }
        finishRequest(
            expected,
            with: .failure(
                SupervisorError.unexpectedResponseIdentifier(
                    expected: expected,
                    actual: identifier
                )
            )
        )
    }

    private func finishCorrelatedOrOnlyRequest(
        identifier: String?,
        error: SupervisorError
    ) {
        if let identifier, pendingResponses[identifier] != nil {
            finishRequest(identifier, with: .failure(error))
            return
        }
        if identifier != nil {
            return
        }
        guard pendingResponses.count == 1, let expected = pendingResponses.keys.first else {
            return
        }
        finishRequest(expected, with: .failure(error))
    }

    private func failUncorrelatedMessage(_ error: SupervisorError) {
        for identifier in Array(pendingResponses.keys) {
            finishRequest(identifier, with: .failure(error))
        }
    }

    private func failOperationMessage(_ line: Data, error: SupervisorError) {
        let operationID = (try? JSONSerialization.jsonObject(with: line))
            .flatMap { $0 as? [String: Any] }?["operation_id"] as? String
        guard let operationID else {
            return
        }
        let subscription = operationSubscriptions.removeValue(forKey: operationID)
        subscription?.continuation.finish(throwing: error)
        bufferedOperationEvents.removeValue(forKey: operationID)
        operationFailures[operationID] = error
        terminalOperations.insert(operationID)
        if subscription != nil {
            pruneOperation(operationID)
        }
    }

    private func finishRequest(
        _ identifier: String,
        with result: Result<Data, any Error>
    ) {
        guard let pending = pendingResponses.removeValue(forKey: identifier) else {
            return
        }
        pending.timeoutTask.cancel()
        pending.continuation.resume(with: result)
    }

    private func timeOutRequest(_ identifier: String) {
        finishRequest(
            identifier,
            with: .failure(SupervisorError.requestTimedOut(identifier))
        )
    }

    private func cancelRequest(_ identifier: String) {
        finishRequest(identifier, with: .failure(CancellationError()))
    }

    private func nextRequestIdentifier() -> String {
        requestSequence &+= 1
        return "macos-\(requestSequence)"
    }

    private func removeOperationSubscription(
        _ operationID: String,
        token: UUID
    ) {
        guard operationSubscriptions[operationID]?.token == token else {
            return
        }
        operationSubscriptions.removeValue(forKey: operationID)
        if terminalOperations.contains(operationID) {
            pruneOperation(operationID)
        }
    }

    private func pruneOperation(_ operationID: String) {
        bufferedOperationEvents.removeValue(forKey: operationID)
        expectedEventSequences.removeValue(forKey: operationID)
        operationFailures.removeValue(forKey: operationID)
        terminalOperations.remove(operationID)
        knownOperations.remove(operationID)
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
        observedTermination = ObservedTermination(
            termination: termination,
            wasRequested: stopRequested
        )
        if !responseStreamHasEnded {
            let timeout = terminationTimeout
            terminationDrainTask?.cancel()
            terminationDrainTask = Task { [weak self] in
                try? await Task.sleep(for: timeout)
                guard !Task.isCancelled else {
                    return
                }
                await self?.finishTerminationAfterDrainTimeout(
                    generation: terminatedGeneration
                )
            }
        }
        finalizeTerminationIfReady()
    }

    private func finishTerminationAfterDrainTimeout(generation timedGeneration: UInt64) {
        guard timedGeneration == generation, observedTermination != nil else {
            return
        }
        responseStreamHasEnded = true
        finalizeTerminationIfReady()
    }

    private func responseStreamDidEnd(generation endedGeneration: UInt64) {
        responseStreamHasEnded = true
        if let process {
            observeTerminationIfNeeded(for: process)
        }
        if observedTermination != nil {
            finalizeTerminationIfReady()
            return
        }

        responseEOFTask?.cancel()
        let responseEOFGrace = responseEOFGrace
        responseEOFTask = Task { [weak self] in
            try? await Task.sleep(for: responseEOFGrace)
            guard !Task.isCancelled else {
                return
            }
            await self?.finishConnectionAfterEOFGrace(
                generation: endedGeneration
            )
        }
    }

    private func finishConnectionAfterEOFGrace(generation endedGeneration: UInt64) {
        guard endedGeneration == generation, responseStreamHasEnded else {
            return
        }
        responseEOFTask = nil
        if let process {
            observeTerminationIfNeeded(for: process)
        }
        terminateProtocolConnectionIfNeeded()
        finalizeTerminationIfReady()
    }

    private func finalizeTerminationIfReady() {
        guard responseStreamHasEnded, let observedTermination else {
            return
        }

        responseEOFTask?.cancel()
        responseEOFTask = nil
        terminationDrainTask?.cancel()
        terminationDrainTask = nil
        terminateProtocolConnectionIfNeeded()
        lineReader?.stop()
        lineReader = nil
        writer?.close { _ in }
        writer = nil
        process = nil
        standardInput = nil
        standardOutput = nil
        stopRequested = false
        handshakeRequested = false
        lastTermination = observedTermination.termination
        state = observedTermination.wasRequested
            ? .idle
            : .exitedUnexpectedly(observedTermination.termination)

        let waiters = terminationWaiters.values
        terminationWaiters.removeAll()
        for waiter in waiters {
            waiter.resume(returning: observedTermination.termination)
        }
        self.observedTermination = nil
    }

    private func terminateProtocolConnectionIfNeeded() {
        guard responseStreamHasEnded, protocolConnectionError == nil else {
            return
        }

        let connectionError: SupervisorError
        if let observedTermination {
            connectionError = observedTermination.wasRequested
                ? .responseStreamEnded
                : .unexpectedTermination(observedTermination.termination)
        } else {
            connectionError = .responseStreamEnded
        }
        protocolConnectionError = connectionError
        lineReader?.stop()

        for identifier in Array(pendingResponses.keys) {
            finishRequest(identifier, with: .failure(connectionError))
        }

        let subscribedOperationIDs = Set(operationSubscriptions.keys)
        for subscription in operationSubscriptions.values {
            subscription.continuation.finish(throwing: connectionError)
        }
        operationSubscriptions.removeAll()
        for operationID in subscribedOperationIDs {
            pruneOperation(operationID)
        }

        let unfinishedOperationIDs = knownOperations
            .union(bufferedOperationEvents.keys)
            .subtracting(terminalOperations)
        for operationID in unfinishedOperationIDs {
            operationFailures[operationID] = connectionError
            terminalOperations.insert(operationID)
        }
    }

    private func resetProtocolState() {
        pendingResponses.removeAll()
        operationSubscriptions.removeAll()
        bufferedOperationEvents.removeAll()
        expectedEventSequences.removeAll()
        knownOperations.removeAll()
        terminalOperations.removeAll()
        operationFailures.removeAll()
        pendingReadBatches.removeAll()
        nextReadBatch = 0
        responseStreamHasEnded = false
        protocolConnectionError = nil
        observedTermination = nil
        responseEOFTask?.cancel()
        responseEOFTask = nil
        terminationDrainTask?.cancel()
        terminationDrainTask = nil
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

private final class ProtocolWriter: @unchecked Sendable {
    private let queue = DispatchQueue(label: "Souvenirs.EngineProtocolWriter")
    private let fileHandle: FileHandle
    private var closed = false

    init(fileHandle: FileHandle) {
        self.fileHandle = fileHandle
    }

    func write(
        _ data: Data,
        completion: @escaping @Sendable ((any Error)?) -> Void
    ) {
        queue.async {
            guard !self.closed else {
                completion(CocoaError(.fileWriteUnknown))
                return
            }
            do {
                try self.fileHandle.write(contentsOf: data)
                completion(nil)
            } catch {
                completion(error)
            }
        }
    }

    func close() async throws {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<Void, any Error>) in
            close { error in
                if let error {
                    continuation.resume(throwing: error)
                } else {
                    continuation.resume()
                }
            }
        }
    }

    func close(
        completion: @escaping @Sendable ((any Error)?) -> Void
    ) {
        queue.async {
            guard !self.closed else {
                completion(nil)
                return
            }
            do {
                try self.fileHandle.close()
                self.closed = true
                completion(nil)
            } catch {
                completion(error)
            }
        }
    }
}

private final class ProtocolLineReader: @unchecked Sendable {
    private let lock = NSLock()
    private var buffer = Data()
    private weak var fileHandle: FileHandle?
    private var onBatch: (@Sendable (UInt64, [Data], Bool) -> Void)?
    private var sequence: UInt64 = 0
    private var ended = false

    func start(
        fileHandle: FileHandle,
        onBatch: @escaping @Sendable (UInt64, [Data], Bool) -> Void
    ) {
        lock.lock()
        self.fileHandle = fileHandle
        self.onBatch = onBatch
        fileHandle.readabilityHandler = { [weak self] readableHandle in
            self?.consumeAvailableData(from: readableHandle)
        }
        lock.unlock()
    }

    func stop() {
        lock.lock()
        ended = true
        fileHandle?.readabilityHandler = nil
        fileHandle = nil
        onBatch = nil
        lock.unlock()
    }

    private func consumeAvailableData(from fileHandle: FileHandle) {
        let data = fileHandle.availableData
        var lines: [Data] = []
        var batchHandler: (@Sendable (UInt64, [Data], Bool) -> Void)?
        var batchSequence: UInt64 = 0
        var reachedEnd = false

        lock.lock()
        guard !ended else {
            lock.unlock()
            return
        }
        if data.isEmpty {
            ended = true
            fileHandle.readabilityHandler = nil
            reachedEnd = true
        } else {
            buffer.append(data)
            while let newlineIndex = buffer.firstIndex(of: 0x0A) {
                lines.append(Data(buffer[..<newlineIndex]))
                buffer.removeSubrange(...newlineIndex)
            }
        }
        batchSequence = sequence
        sequence &+= 1
        batchHandler = onBatch
        if reachedEnd {
            onBatch = nil
        }
        lock.unlock()

        batchHandler?(batchSequence, lines, reachedEnd)
    }
}

private struct RequestEnvelope<Parameters: Encodable>: Encodable {
    let id: String
    let protocolVersion: UInt32
    let method: String
    let params: Parameters

    enum CodingKeys: String, CodingKey {
        case id
        case protocolVersion = "protocol_version"
        case method
        case params
    }
}

private struct HandshakeParameters: Encodable, Sendable {
    let clientVersion: String

    enum CodingKeys: String, CodingKey {
        case clientVersion = "client_version"
    }
}

private struct CancelOperationParameters: Encodable, Sendable {
    let operationID: String

    enum CodingKeys: String, CodingKey {
        case operationID = "operation_id"
    }
}

private struct CatalogValidationParameters: Encodable, Sendable {
    let path: String
    let intent: String
}

private struct CatalogPathParameters: Encodable, Sendable {
    let path: String
}

private struct CreateCatalogParameters: Encodable, Sendable {
    let path: String
    let displayName: String

    enum CodingKeys: String, CodingKey {
        case path
        case displayName = "display_name"
    }
}

private struct ImportScanParameters: Encodable, Sendable {
    let sourcePath: String

    enum CodingKeys: String, CodingKey {
        case sourcePath = "source_path"
    }
}

private struct ImportReviewParameters: Encodable, Sendable {
    let catalogPath: String
    let sourcePath: String
    let collectionName: String
    let destinationMode: EngineSupervisor.ImportDestinationMode
    let acceptedDate: String
    let dateResolution: EngineSupervisor.ImportDateResolution?

    enum CodingKeys: String, CodingKey {
        case catalogPath = "catalog_path"
        case sourcePath = "source_path"
        case collectionName = "collection_name"
        case destinationMode = "destination_mode"
        case acceptedDate = "accepted_date"
        case dateResolution = "date_resolution"
    }
}

private struct SuccessResponse<Result: Decodable>: Decodable {
    let kind: String
    let id: String?
    let protocolVersion: UInt32
    let status: String
    let result: Result

    enum CodingKeys: String, CodingKey {
        case kind
        case id
        case protocolVersion = "protocol_version"
        case status
        case result
    }
}

private struct ErrorResponse: Decodable {
    let error: ProtocolErrorResponse
}

private struct HandshakeResult: Decodable, Sendable {
    let type: String
    let engineVersion: String
    let protocolVersion: UInt32
    let compatibleClientVersion: String
    let capabilities: [String]

    enum CodingKeys: String, CodingKey {
        case type
        case engineVersion = "engine_version"
        case protocolVersion = "protocol_version"
        case compatibleClientVersion = "compatible_client_version"
        case capabilities
    }
}

private struct OperationAcceptedResult: Decodable, Sendable {
    let type: String
    let operationID: String

    enum CodingKeys: String, CodingKey {
        case type
        case operationID = "operation_id"
    }
}

private struct CancellationAcceptedResult: Decodable, Sendable {
    let type: String
    let operationID: String

    enum CodingKeys: String, CodingKey {
        case type
        case operationID = "operation_id"
    }
}

private struct CatalogResult: Decodable, Sendable {
    let type: String
    let path: String
    let volumeName: String
    let displayName: String?
    let detail: String

    enum CodingKeys: String, CodingKey {
        case type
        case path
        case volumeName = "volume_name"
        case displayName = "display_name"
        case detail
    }
}

private struct ImportScanResult: Decodable, Sendable {
    let type: String
    let sourcePath: String
    let sourceName: String
    let fileCount: Int
    let mediaCount: Int
    let photoCount: Int
    let videoCount: Int
    let unsupportedCount: Int
    let uncertainTimeCount: Int
    let totalBytes: UInt64
    let suggestedCollectionName: String
    let suggestedAcceptedDate: String

    var scan: EngineSupervisor.ImportScan {
        .init(
            sourcePath: sourcePath,
            sourceName: sourceName,
            fileCount: fileCount,
            mediaCount: mediaCount,
            photoCount: photoCount,
            videoCount: videoCount,
            unsupportedCount: unsupportedCount,
            uncertainTimeCount: uncertainTimeCount,
            totalBytes: totalBytes,
            suggestedCollectionName: suggestedCollectionName,
            suggestedAcceptedDate: suggestedAcceptedDate
        )
    }

    enum CodingKeys: String, CodingKey {
        case type
        case sourcePath = "source_path"
        case sourceName = "source_name"
        case fileCount = "file_count"
        case mediaCount = "media_count"
        case photoCount = "photo_count"
        case videoCount = "video_count"
        case unsupportedCount = "unsupported_count"
        case uncertainTimeCount = "uncertain_time_count"
        case totalBytes = "total_bytes"
        case suggestedCollectionName = "suggested_collection_name"
        case suggestedAcceptedDate = "suggested_accepted_date"
    }
}

private struct ImportReviewResult: Decodable, Sendable {
    let type: String
    let scan: EngineSupervisor.ImportScan
    let collectionName: String
    let destinationMode: EngineSupervisor.ImportDestinationMode
    let destinationPath: String
    let acceptedDate: String
    let dateResolution: EngineSupervisor.ImportDateResolution?
    let requiredDecisionCount: Int
    let transferCount: Int
    let skippedCount: Int
    let leftOnSourceCount: Int
    let operationCount: Int
    let requiredSpaceBytes: UInt64
    let canStart: Bool

    var review: EngineSupervisor.ImportReview {
        .init(
            scan: scan,
            collectionName: collectionName,
            destinationMode: destinationMode,
            destinationPath: destinationPath,
            acceptedDate: acceptedDate,
            dateResolution: dateResolution,
            requiredDecisionCount: requiredDecisionCount,
            transferCount: transferCount,
            skippedCount: skippedCount,
            leftOnSourceCount: leftOnSourceCount,
            operationCount: operationCount,
            requiredSpaceBytes: requiredSpaceBytes,
            canStart: canStart
        )
    }

    enum CodingKeys: String, CodingKey {
        case type
        case scan
        case collectionName = "collection_name"
        case destinationMode = "destination_mode"
        case destinationPath = "destination_path"
        case acceptedDate = "accepted_date"
        case dateResolution = "date_resolution"
        case requiredDecisionCount = "required_decision_count"
        case transferCount = "transfer_count"
        case skippedCount = "skipped_count"
        case leftOnSourceCount = "left_on_source_count"
        case operationCount = "operation_count"
        case requiredSpaceBytes = "required_space_bytes"
        case canStart = "can_start"
    }
}

private struct OperationInspectionResult: Decodable, Sendable {
    let type: String
    let operation: EngineSupervisor.UnfinishedOperation?
}

private struct OperationEventEnvelope: Decodable {
    let kind: String
    let operationID: String
    let sequence: UInt64
    let event: String
    let name: String?
    let completedUnits: UInt64?
    let totalUnits: UInt64?
    let message: String?
    let error: ProtocolErrorResponse?

    enum CodingKeys: String, CodingKey {
        case kind
        case operationID = "operation_id"
        case sequence
        case event
        case name
        case completedUnits = "completed_units"
        case totalUnits = "total_units"
        case message
        case error
    }
}

private struct ProtocolErrorResponse: Decodable {
    let code: String
    let message: String
}
