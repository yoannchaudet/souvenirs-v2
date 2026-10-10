import Foundation

@MainActor
protocol ImportExecutionEngine: AnyObject {
    func startImportExecution(
        catalogPath: String,
        sourcePath: String,
        collectionName: String,
        destinationMode: EngineSupervisor.ImportDestinationMode,
        acceptedDate: String,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> String
    func inspectUnfinishedOperation(catalogPath: String) async throws
        -> EngineSupervisor.UnfinishedOperation?
    func recoverOperation(catalogPath: String, operationID: String) async throws -> String
    func cancel(operationID: String) async throws -> EngineSupervisor.CancellationAccepted
    func events(
        for operationID: String
    ) async throws -> AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error>
}

extension EngineAppModel: ImportExecutionEngine {
    func startImportExecution(
        catalogPath: String,
        sourcePath: String,
        collectionName: String,
        destinationMode: EngineSupervisor.ImportDestinationMode,
        acceptedDate: String,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> String {
        try await supervisor.requestOperation(
            method: "operation.start",
            parameters: StartImportParameters(
                catalogPath: catalogPath,
                sourcePath: sourcePath,
                collectionName: collectionName,
                destinationMode: destinationMode,
                acceptedDate: acceptedDate,
                dateResolution: dateResolution
            )
        )
    }

    func inspectUnfinishedOperation(
        catalogPath: String
    ) async throws -> EngineSupervisor.UnfinishedOperation? {
        try await supervisor.inspectUnfinishedOperation(catalogPath: catalogPath)
    }

    func recoverOperation(catalogPath: String, operationID: String) async throws -> String {
        try await supervisor.requestOperation(
            method: "operation.start",
            parameters: RecoverOperationParameters(
                catalogPath: catalogPath,
                operationID: operationID
            )
        )
    }

    func cancel(
        operationID: String
    ) async throws -> EngineSupervisor.CancellationAccepted {
        try await supervisor.cancel(operationID: operationID)
    }

    func events(
        for operationID: String
    ) async throws -> AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error> {
        try await supervisor.events(for: operationID)
    }

}

struct ImportOutcome: Equatable {
    let copiedCount: Int
    let skippedCount: Int
    let leftOnSourceCount: Int
}

@MainActor
final class ImportExecutionCoordinator: ObservableObject {
    enum Phase: Equatable {
        case idle
        case running
        case recoveryPrompt(sourceAvailable: Bool)
        case recovering
        case stopping
        case sourceUnavailable
        case completed(ImportOutcome)
        case cancelled(copiedCount: Int)
        case failed(String)
    }

    @Published private(set) var phase: Phase = .idle
    @Published private(set) var completedUnits: UInt64 = 0
    @Published private(set) var totalUnits: UInt64 = 1
    @Published private(set) var progressMessage = "Preparing verified copy…"
    @Published private(set) var operationID: String?
    @Published private(set) var catalog: RecentCatalog?
    @Published private(set) var review: EngineSupervisor.ImportReview?
    @Published private(set) var sourceName = ""
    @Published private(set) var collectionName = ""

    private let engine: ImportExecutionEngine
    private var operationTask: Task<Void, Never>?
    private var recovery: EngineSupervisor.UnfinishedOperation?
    private var inspectionGeneration = 0

    init(engine: ImportExecutionEngine) {
        self.engine = engine
    }

    var fractionCompleted: Double {
        guard totalUnits > 0 else { return 0 }
        return min(Double(completedUnits) / Double(totalUnits), 1)
    }

    var percentCompleted: Int {
        Int((fractionCompleted * 100).rounded())
    }

    var canStop: Bool { operationID != nil }

    func start(
        review: EngineSupervisor.ImportReview,
        catalog: RecentCatalog
    ) {
        operationTask?.cancel()
        inspectionGeneration += 1
        operationID = nil
        self.review = review
        self.catalog = catalog
        sourceName = review.scan.sourceName
        collectionName = review.collectionName
        completedUnits = 0
        totalUnits = UInt64(max(review.transferCount, 1))
        progressMessage = "Preparing verified copy…"
        phase = .running
        operationTask = Task {
            do {
                let operationID = try await engine.startImportExecution(
                    catalogPath: catalog.path,
                    sourcePath: review.scan.sourcePath,
                    collectionName: review.collectionName,
                    destinationMode: review.destinationMode,
                    acceptedDate: review.acceptedDate,
                    dateResolution: review.dateResolution
                )
                self.operationID = operationID
                try await consume(operationID: operationID, recovering: false)
            } catch {
                guard !Task.isCancelled else { return }
                phase = failurePhase(error)
            }
        }
    }

    func inspect(catalog: RecentCatalog) async {
        guard phase == .idle else { return }
        inspectionGeneration += 1
        let generation = inspectionGeneration
        do {
            let unfinished = try await engine.inspectUnfinishedOperation(
                catalogPath: catalog.path
            )
            guard phase == .idle, generation == inspectionGeneration else { return }
            guard let unfinished else { return }
            recovery = unfinished
            self.catalog = catalog
            sourceName = unfinished.sourceName
            collectionName = unfinished.collectionName
            completedUnits = UInt64(unfinished.copiedCount)
            totalUnits = UInt64(max(unfinished.totalCount, 1))
            phase = .recoveryPrompt(sourceAvailable: unfinished.sourceAvailable)
        } catch {
            guard phase == .idle, generation == inspectionGeneration else { return }
            self.catalog = catalog
            phase = .failed(userMessage(for: error))
        }
    }

    func finishRecovery() {
        guard recovery != nil else { return }
        phase = .recovering
        progressMessage = "Checking copied files…"
        operationTask = Task {
            do {
                guard let recovery else { return }
                guard let catalog else { return }
                let operationID = try await engine.recoverOperation(
                    catalogPath: catalog.path,
                    operationID: recovery.operationID
                )
                self.operationID = operationID
                try await consume(operationID: operationID, recovering: true)
            } catch {
                guard !Task.isCancelled else { return }
                phase = failurePhase(error)
            }
        }
    }

    func deferRecovery() {
        dismissOutcome()
    }

    func stop() {
        guard let operationID else { return }
        phase = .stopping
        Task {
            do {
                _ = try await engine.cancel(operationID: operationID)
            } catch {
                if case .completed = phase {
                    return
                }
                if case .stopping = phase {
                    return
                }
                phase = failurePhase(error)
            }
        }
    }

    func stopRecoveryHere() {
        guard let recovery else { return }
        operationID = recovery.operationID
        phase = .stopping
        Task {
            do {
                _ = try await engine.cancel(operationID: recovery.operationID)
                phase = .cancelled(copiedCount: recovery.copiedCount)
            } catch {
                phase = failurePhase(error)
            }
        }
    }

    func dismissOutcome() {
        phase = .idle
        operationID = nil
        recovery = nil
        catalog = nil
        review = nil
        sourceName = ""
        collectionName = ""
    }

    private func consume(operationID: String, recovering: Bool) async throws {
        let events = try await engine.events(for: operationID)
        for try await event in events {
            guard !Task.isCancelled else { return }
            switch event.payload {
            case .started:
                progressMessage = recovering
                    ? "Checking copied files…"
                    : "Copying and checking each file…"
            case .progress(let completed, let total, let message):
                completedUnits = completed
                if let total { totalUnits = max(total, 1) }
                if let message { progressMessage = message }
            case .completed(let result):
                do {
                    phase = .completed(try decodeOutcome(result))
                } catch {
                    phase = .failed("The engine returned an invalid completion result.")
                }
            case .failed(let code, let message):
                phase = code == "source_unavailable"
                    ? .sourceUnavailable : .failed(message)
            case .cancelled:
                phase = .cancelled(copiedCount: Int(completedUnits))
            }
        }
        if phase == .running || phase == .recovering || phase == .stopping {
            phase = .failed("The engine stopped sending updates.")
        }
    }

    private func decodeOutcome(_ data: Data) throws -> ImportOutcome {
        struct Payload: Decodable {
            let copiedCount: Int
            let skippedCount: Int
            let leftOnSourceCount: Int

            enum CodingKeys: String, CodingKey {
                case copiedCount = "copied_count"
                case skippedCount = "skipped_count"
                case leftOnSourceCount = "left_on_source_count"
            }
        }
        let payload = try JSONDecoder().decode(Payload.self, from: data)
        return ImportOutcome(
            copiedCount: payload.copiedCount,
            skippedCount: payload.skippedCount,
            leftOnSourceCount: payload.leftOnSourceCount
        )
    }

    private func failurePhase(_ error: Error) -> Phase {
        if case EngineSupervisor.SupervisorError.protocolError(let code, _) = error,
           code == "source_unavailable" {
            return .sourceUnavailable
        }
        return .failed(userMessage(for: error))
    }

    private func userMessage(for error: Error) -> String {
        if case EngineSupervisor.SupervisorError.protocolError(_, let message) = error {
            return message
        }
        return error.localizedDescription
    }
}

private struct StartImportParameters: Encodable, Sendable {
    let name = "import.execute"
    let catalogPath: String
    let sourcePath: String
    let collectionName: String
    let destinationMode: EngineSupervisor.ImportDestinationMode
    let acceptedDate: String
    let dateResolution: EngineSupervisor.ImportDateResolution?

    enum CodingKeys: String, CodingKey {
        case name
        case catalogPath = "catalog_path"
        case sourcePath = "source_path"
        case collectionName = "collection_name"
        case destinationMode = "destination_mode"
        case acceptedDate = "accepted_date"
        case dateResolution = "date_resolution"
    }
}

private struct RecoverOperationParameters: Encodable, Sendable {
    let name = "operation.recover"
    let catalogPath: String
    let operationID: String

    enum CodingKeys: String, CodingKey {
        case name
        case catalogPath = "catalog_path"
        case operationID = "operation_id"
    }
}
