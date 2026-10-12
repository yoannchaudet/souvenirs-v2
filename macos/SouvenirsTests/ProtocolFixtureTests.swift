import Foundation
import XCTest
@testable import Souvenirs

final class ProtocolFixtureTests: XCTestCase {
    func testSharedGoldenFixtureMatchesEverySwiftProtocolShape() throws {
        let lines = try fixtureLines()

        let actual = try lines.enumerated().map { index, line in
            let data = Data(line.utf8)
            let shape = try ProtocolFixtureValidator.validate(
                data,
                lineNumber: index + 1
            )
            try validateWithProductionProtocol(data, at: index)
            return shape
        }

        XCTAssertEqual(
            actual,
            [
                .request(id: "macos-handshake-1", method: "handshake"),
                .successResponse(id: "macos-handshake-1", result: "handshake"),
                .request(id: "request-2", method: "operation.cancel"),
                .successResponse(id: "request-2", result: "cancellation_accepted"),
                .request(id: "request-3", method: "operation.start"),
                .successResponse(id: "request-3", result: "operation_accepted"),
                .request(id: "request-4", method: "operation.inspect"),
                .request(id: "request-4b", method: "operation.start"),
                .successResponse(id: "request-4", result: "operation_inspection"),
                .request(id: "request-collections", method: "catalog.collections"),
                .successResponse(id: "request-collections", result: "catalog_collections"),
                .request(id: "request-browse", method: "catalog.browse"),
                .successResponse(id: "request-browse", result: "catalog_browse"),
                .request(id: "request-review", method: "import.review"),
                .successResponse(id: "request-review", result: "import_review"),
                .errorResponse(id: "request-5", code: "operation_not_found"),
                .event(operationID: "operation-1", event: "started"),
                .event(operationID: "operation-1", event: "progress"),
                .event(operationID: "operation-1", event: "completed"),
                .event(operationID: "operation-2", event: "failed"),
                .event(operationID: "operation-3", event: "cancelled"),
            ]
        )
    }

    func testFixtureValidationRejectsNonCanonicalFieldName() {
        assertRejected(
            """
            {"id":"macos-handshake-1","protocol_version":7,"method":"handshake","params":{"clientVersion":"0.2.0"}}
            """,
            containing: "keys were"
        )
    }

    func testFixtureValidationRejectsUnknownEnumValue() {
        assertRejected(
            """
            {"kind":"event","protocol_version":7,"operation_id":"operation-1","sequence":0,"event":"advanced","name":"import.scan"}
            """,
            containing: "unsupported event type"
        )
    }

    func testFixtureValidationRejectsIncompatibleMessageShape() {
        assertRejected(
            """
            {"kind":"response","id":"request-2","protocol_version":7,"status":"success","result":[]}
            """,
            containing: "must be an object"
        )
    }

    func testFixtureValidationRejectsJSONTypeDrift() {
        assertRejected(
            """
            {"kind":"event","protocol_version":7,"operation_id":"operation-1","sequence":true,"event":"started","name":"import.scan"}
            """,
            containing: "must be an integer"
        )
    }

    private var fixtureURL: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("crates/souvenirs-protocol/fixtures/protocol-v7.jsonl")
    }

    private func assertRejected(
        _ line: String,
        containing expectedMessage: String,
        file: StaticString = #filePath,
        line sourceLine: UInt = #line
    ) {
        XCTAssertThrowsError(
            try ProtocolFixtureValidator.validate(Data(line.utf8), lineNumber: 1),
            file: file,
            line: sourceLine
        ) { error in
            XCTAssertTrue(
                String(describing: error).contains(expectedMessage),
                "Expected \(error) to contain \(expectedMessage)",
                file: file,
                line: sourceLine
            )
        }
    }

    private func fixtureLines() throws -> [Substring] {
        try String(contentsOf: fixtureURL, encoding: .utf8)
            .split(separator: "\n")
    }

    private func validateWithProductionProtocol(
        _ data: Data,
        at index: Int
    ) throws {
        switch index {
        case 0:
            try assertRequest(
                data,
                equals: RequestEnvelope(
                    id: "macos-handshake-1",
                    protocolVersion: EngineSupervisor.protocolVersion,
                    method: "handshake",
                    params: HandshakeParameters(
                        clientVersion: EngineSupervisor.protocolClientVersion
                    )
                )
            )
        case 1:
            let response = try JSONDecoder().decode(
                SuccessResponse<HandshakeResult>.self,
                from: data
            )
            XCTAssertEqual(response.kind, "response")
            XCTAssertEqual(response.id, "macos-handshake-1")
            XCTAssertEqual(response.protocolVersion, 7)
            XCTAssertEqual(response.status, "success")
            XCTAssertEqual(response.result.type, "handshake")
            XCTAssertEqual(response.result.engineVersion, "0.1.0")
            XCTAssertEqual(response.result.protocolVersion, 7)
            XCTAssertEqual(response.result.compatibleClientVersion, "0.2.0")
            XCTAssertEqual(
                response.result.capabilities,
                [
                    "operation.cancel",
                    "catalog.browse",
                    "catalog.collections",
                    "import.review",
                ]
            )
        case 2:
            try assertRequest(
                data,
                equals: RequestEnvelope(
                    id: "request-2",
                    protocolVersion: EngineSupervisor.protocolVersion,
                    method: "operation.cancel",
                    params: CancelOperationParameters(operationID: "operation-1")
                )
            )
        case 3:
            let response = try JSONDecoder().decode(
                SuccessResponse<CancellationAcceptedResult>.self,
                from: data
            )
            XCTAssertEqual(response.result.type, "cancellation_accepted")
            XCTAssertEqual(response.result.operationID, "operation-1")
        case 4:
            try assertRequest(
                data,
                equals: RequestEnvelope(
                    id: "request-3",
                    protocolVersion: EngineSupervisor.protocolVersion,
                    method: "operation.start",
                    params: StartImportParameters(
                        reviewedPlan:
                            #"{"schema_version":1,"digest":"reviewed-digest","plan":{}}"#
                    )
                )
            )
        case 5:
            let response = try JSONDecoder().decode(
                SuccessResponse<OperationAcceptedResult>.self,
                from: data
            )
            XCTAssertEqual(response.result.type, "operation_accepted")
            XCTAssertEqual(response.result.operationID, "operation-1")
        case 6:
            try assertRequest(
                data,
                equals: RequestEnvelope(
                    id: "request-4",
                    protocolVersion: EngineSupervisor.protocolVersion,
                    method: "operation.inspect",
                    params: CatalogPathParameters(
                        path: "/Volumes/photos/Family Catalog"
                    )
                )
            )
        case 7:
            try assertRequest(
                data,
                equals: RequestEnvelope(
                    id: "request-4b",
                    protocolVersion: EngineSupervisor.protocolVersion,
                    method: "operation.start",
                    params: RecoverOperationParameters(
                        catalogPath: "/Volumes/photos/Family Catalog",
                        operationID: "operation-1"
                    )
                )
            )
        case 8:
            let response = try JSONDecoder().decode(
                SuccessResponse<OperationInspectionResult>.self,
                from: data
            )
            XCTAssertEqual(response.result.type, "operation_inspection")
            XCTAssertEqual(
                response.result.operation,
                .init(
                    operationID: "operation-1",
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
            )
        case 9:
            try assertRequest(
                data,
                equals: RequestEnvelope(
                    id: "request-collections",
                    protocolVersion: EngineSupervisor.protocolVersion,
                    method: "catalog.collections",
                    params: CatalogPathParameters(
                        path: "/Volumes/photos/Family Catalog"
                    )
                )
            )
        case 10:
            let response = try JSONDecoder().decode(
                SuccessResponse<CatalogCollectionsResult>.self,
                from: data
            )
            XCTAssertEqual(response.result.type, "catalog_collections")
            XCTAssertEqual(
                response.result.collections,
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
                            .init(
                                year: 2020,
                                yearDirectory: "2020",
                                collectionDirectory: "z Kids"
                            ),
                        ]
                    ),
                ]
            )
        case 11:
            try assertRequest(
                data,
                equals: RequestEnvelope(
                    id: "request-browse",
                    protocolVersion: EngineSupervisor.protocolVersion,
                    method: "catalog.browse",
                    params: CatalogBrowseParameters(
                        path: "/Volumes/photos/Family Catalog",
                        selection: .ongoing(key: "kids")
                    )
                )
            )
        case 12:
            let response = try JSONDecoder().decode(
                SuccessResponse<CatalogBrowseResult>.self,
                from: data
            )
            XCTAssertEqual(response.result.type, "catalog_browse")
            XCTAssertEqual(response.result.collections.count, 1)
            guard case .ongoing(
                identity: .ongoing(key: "kids"),
                label: "Kids",
                mediaCount: 1,
                parts: let parts
            ) = response.result.collections[0] else {
                return XCTFail("Expected ongoing browse collection")
            }
            XCTAssertEqual(parts[0].dateGroups[0].date, "2019-03-02")
            XCTAssertEqual(parts[1].problem, "invalid catalog manifest")
        case 13:
            try assertRequest(
                data,
                equals: RequestEnvelope(
                    id: "request-review",
                    protocolVersion: EngineSupervisor.protocolVersion,
                    method: "import.review",
                    params: ImportReviewParameters(
                        catalogPath: "/Volumes/photos/Family Catalog",
                        sourcePath: "/Volumes/EOS_DIGITAL",
                        destination: .existing(
                            identity: .dated(
                                year: "2019",
                                directory: "2019-07-14 +6d Brittany"
                            )
                        ),
                        dateResolution: .asShown,
                        utcOffsetSeconds: 7_200
                    )
                )
            )
        case 14:
            let response = try JSONDecoder().decode(
                SuccessResponse<ImportReviewResult>.self,
                from: data
            )
            XCTAssertEqual(response.result.type, "import_review")
            XCTAssertEqual(
                response.result.review.destination,
                .existing(
                    identity: .dated(
                        year: "2019",
                        directory: "2019-07-14 +6d Brittany"
                    )
                )
            )
            XCTAssertEqual(response.result.review.collectionName, "Brittany")
            XCTAssertEqual(
                response.result.review.destinationPath,
                "/Volumes/photos/Family Catalog/2019/2019-07-14 +6d Brittany"
            )
        case 15:
            let response = try JSONDecoder().decode(ErrorResponse.self, from: data)
            XCTAssertEqual(response.error.code, "operation_not_found")
            XCTAssertEqual(response.error.message, "The operation was not found.")
        case 16:
            XCTAssertEqual(
                try EngineSupervisor.decodeOperationEvent(data),
                .init(
                    operationID: "operation-1",
                    sequence: 0,
                    payload: .started(name: "import.scan")
                )
            )
        case 17:
            XCTAssertEqual(
                try EngineSupervisor.decodeOperationEvent(data),
                .init(
                    operationID: "operation-1",
                    sequence: 1,
                    payload: .progress(
                        completedUnits: 1,
                        totalUnits: 2,
                        message: "Scanning"
                    )
                )
            )
        case 18:
            let event = try EngineSupervisor.decodeOperationEvent(data)
            guard case .completed(let result) = event.payload else {
                return XCTFail("Expected a completed event")
            }
            XCTAssertEqual(event.operationID, "operation-1")
            XCTAssertEqual(event.sequence, 2)
            XCTAssertEqual(
                try JSONSerialization.jsonObject(with: result) as? [String: Int],
                ["item_count": 1]
            )
        case 19:
            XCTAssertEqual(
                try EngineSupervisor.decodeOperationEvent(data),
                .init(
                    operationID: "operation-2",
                    sequence: 0,
                    payload: .failed(
                        code: "source_unavailable",
                        message: "The source is unavailable."
                    )
                )
            )
        case 20:
            XCTAssertEqual(
                try EngineSupervisor.decodeOperationEvent(data),
                .init(
                    operationID: "operation-3",
                    sequence: 0,
                    payload: .cancelled(copiedCount: 1)
                )
            )
        default:
            XCTFail("Unexpected fixture line \(index + 1)")
        }
    }

    private func assertRequest<Parameters: Encodable>(
        _ fixture: Data,
        equals request: RequestEnvelope<Parameters>
    ) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        XCTAssertEqual(
            try canonicalJSON(fixture),
            try canonicalJSON(encoder.encode(request))
        )
    }

    private func canonicalJSON(_ data: Data) throws -> Data {
        try JSONSerialization.data(
            withJSONObject: JSONSerialization.jsonObject(with: data),
            options: [.sortedKeys]
        )
    }
}

private enum ProtocolFixtureShape: Equatable {
    case request(id: String, method: String)
    case successResponse(id: String, result: String)
    case errorResponse(id: String, code: String)
    case event(operationID: String, event: String)
}

private enum ProtocolFixtureValidator {
    static func validate(
        _ data: Data,
        lineNumber: Int
    ) throws -> ProtocolFixtureShape {
        let value = try JSONSerialization.jsonObject(with: data)
        let object = try requireObject(value, at: "line \(lineNumber)")
        try requireInteger(
            object,
            key: "protocol_version",
            at: "line \(lineNumber)",
            equals: Int(EngineSupervisor.protocolVersion)
        )

        guard let kind = object["kind"] else {
            return try validateRequest(object, lineNumber: lineNumber)
        }
        switch try requireString(kind, at: "line \(lineNumber).kind") {
        case "response":
            return try validateResponse(object, lineNumber: lineNumber)
        case "event":
            return try validateEvent(object, lineNumber: lineNumber)
        default:
            throw ValidationError("line \(lineNumber) has an unsupported message kind")
        }
    }

    private static func validateRequest(
        _ object: [String: Any],
        lineNumber: Int
    ) throws -> ProtocolFixtureShape {
        let path = "line \(lineNumber)"
        try requireKeys(
            object,
            exactly: ["id", "protocol_version", "method", "params"],
            at: path
        )
        let id = try requireString(object, key: "id", at: path)
        let method = try requireString(object, key: "method", at: path)
        let parameters = try requireObject(object, key: "params", at: path)

        switch method {
        case "handshake":
            try requireKeys(parameters, exactly: ["client_version"], at: "\(path).params")
            _ = try requireString(parameters, key: "client_version", at: "\(path).params")
        case "operation.cancel":
            try requireKeys(parameters, exactly: ["operation_id"], at: "\(path).params")
            _ = try requireString(parameters, key: "operation_id", at: "\(path).params")
        case "operation.inspect":
            try requireKeys(parameters, exactly: ["path"], at: "\(path).params")
            _ = try requireString(parameters, key: "path", at: "\(path).params")
        case "catalog.collections":
            try requireKeys(parameters, exactly: ["path"], at: "\(path).params")
            _ = try requireString(parameters, key: "path", at: "\(path).params")
        case "catalog.browse":
            try requireKeys(parameters, exactly: ["path", "selection"], at: "\(path).params")
            _ = try requireString(parameters, key: "path", at: "\(path).params")
            let selection = try requireObject(parameters, key: "selection", at: "\(path).params")
            try requireKeys(selection, exactly: ["kind", "key"], at: "\(path).params.selection")
            _ = try requireString(selection, key: "key", at: "\(path).params.selection")
        case "import.review":
            try requireKeys(
                parameters,
                exactly: [
                    "catalog_path", "source_path", "destination",
                    "date_resolution", "utc_offset_seconds",
                ],
                at: "\(path).params"
            )
            _ = try requireString(parameters, key: "catalog_path", at: "\(path).params")
            _ = try requireString(parameters, key: "source_path", at: "\(path).params")
            try validateDestination(
                try requireObject(parameters, key: "destination", at: "\(path).params"),
                at: "\(path).params.destination"
            )
            _ = try requireString(parameters, key: "date_resolution", at: "\(path).params")
            _ = try requireInteger(
                parameters,
                key: "utc_offset_seconds",
                at: "\(path).params"
            )
        case "operation.start":
            try validateStartParameters(parameters, at: "\(path).params")
        default:
            throw ValidationError("\(path) has an unsupported request method")
        }
        return .request(id: id, method: method)
    }

    private static func validateStartParameters(
        _ parameters: [String: Any],
        at path: String
    ) throws {
        let name = try requireString(parameters, key: "name", at: path)
        switch name {
        case "import.execute":
            try requireKeys(parameters, exactly: ["name", "reviewed_plan"], at: path)
            _ = try requireString(parameters, key: "reviewed_plan", at: path)
        case "operation.recover":
            try requireKeys(
                parameters,
                exactly: ["name", "catalog_path", "operation_id"],
                at: path
            )
            _ = try requireString(parameters, key: "catalog_path", at: path)
            _ = try requireString(parameters, key: "operation_id", at: path)
        default:
            throw ValidationError("\(path) has an unsupported operation name")
        }
    }

    private static func validateResponse(
        _ object: [String: Any],
        lineNumber: Int
    ) throws -> ProtocolFixtureShape {
        let path = "line \(lineNumber)"
        let status = try requireString(object, key: "status", at: path)
        let id = try requireString(object, key: "id", at: path)
        switch status {
        case "success":
            try requireKeys(
                object,
                exactly: ["kind", "id", "protocol_version", "status", "result"],
                at: path
            )
            let result = try requireObject(object, key: "result", at: path)
            let resultType = try validateSuccessResult(result, at: "\(path).result")
            return .successResponse(id: id, result: resultType)
        case "error":
            try requireKeys(
                object,
                exactly: ["kind", "id", "protocol_version", "status", "error"],
                at: path
            )
            let error = try requireObject(object, key: "error", at: path)
            try requireKeys(error, exactly: ["code", "message", "data"], at: "\(path).error")
            let code = try requireString(error, key: "code", at: "\(path).error")
            _ = try requireString(error, key: "message", at: "\(path).error")
            let data = try requireObject(error, key: "data", at: "\(path).error")
            try requireKeys(data, exactly: ["operation_id"], at: "\(path).error.data")
            _ = try requireString(data, key: "operation_id", at: "\(path).error.data")
            return .errorResponse(id: id, code: code)
        default:
            throw ValidationError("\(path) has an unsupported response status")
        }
    }

    private static func validateSuccessResult(
        _ result: [String: Any],
        at path: String
    ) throws -> String {
        let type = try requireString(result, key: "type", at: path)
        switch type {
        case "handshake":
            try requireKeys(
                result,
                exactly: [
                    "type", "engine_version", "protocol_version",
                    "compatible_client_version", "capabilities",
                ],
                at: path
            )
            _ = try requireString(result, key: "engine_version", at: path)
            try requireInteger(
                result,
                key: "protocol_version",
                at: path,
                equals: Int(EngineSupervisor.protocolVersion)
            )
            _ = try requireString(result, key: "compatible_client_version", at: path)
            let capabilities = try requireArray(result, key: "capabilities", at: path)
            for (index, capability) in capabilities.enumerated() {
                _ = try requireString(capability, at: "\(path).capabilities[\(index)]")
            }
        case "cancellation_accepted", "operation_accepted":
            try requireKeys(result, exactly: ["type", "operation_id"], at: path)
            _ = try requireString(result, key: "operation_id", at: path)
        case "operation_inspection":
            try requireKeys(result, exactly: ["type", "operation"], at: path)
            let operation = try requireObject(result, key: "operation", at: path)
            try requireKeys(
                operation,
                exactly: [
                    "operation_id", "collection_name", "source_name", "copied_count",
                    "total_count", "remaining_count", "source_available", "interruption",
                    "activity_time",
                ],
                at: "\(path).operation"
            )
            _ = try requireString(operation, key: "operation_id", at: "\(path).operation")
            _ = try requireString(operation, key: "collection_name", at: "\(path).operation")
            _ = try requireString(operation, key: "source_name", at: "\(path).operation")
            _ = try requireInteger(operation, key: "copied_count", at: "\(path).operation")
            _ = try requireInteger(operation, key: "total_count", at: "\(path).operation")
            _ = try requireInteger(operation, key: "remaining_count", at: "\(path).operation")
            _ = try requireBoolean(operation, key: "source_available", at: "\(path).operation")
            _ = try requireString(operation, key: "interruption", at: "\(path).operation")
            let activityTime = try requireObject(
                operation,
                key: "activity_time",
                at: "\(path).operation"
            )
            try requireKeys(
                activityTime,
                exactly: ["kind", "unix_seconds", "nanosecond"],
                at: "\(path).operation.activity_time"
            )
            _ = try requireString(
                activityTime,
                key: "kind",
                at: "\(path).operation.activity_time"
            )
            _ = try requireInteger(
                activityTime,
                key: "unix_seconds",
                at: "\(path).operation.activity_time"
            )
            _ = try requireInteger(
                activityTime,
                key: "nanosecond",
                at: "\(path).operation.activity_time"
            )
        case "catalog_collections":
            try requireKeys(result, exactly: ["type", "collections"], at: path)
            let collections = try requireArray(result, key: "collections", at: path)
            for (index, value) in collections.enumerated() {
                try validateCollection(value, at: "\(path).collections[\(index)]")
            }
        case "catalog_browse":
            try requireKeys(result, exactly: ["type", "collections"], at: path)
            let collections = try requireArray(result, key: "collections", at: path)
            for (index, value) in collections.enumerated() {
                try validateBrowseCollection(value, at: "\(path).collections[\(index)]")
            }
        case "import_review":
            try requireKeys(
                result,
                exactly: [
                    "type", "scan", "collection_name", "destination", "destination_path",
                    "accepted_date", "date_resolution", "required_decision_count",
                    "transfer_count", "skipped_count", "left_on_source_count",
                    "operation_count", "preflight", "can_start", "reviewed_plan",
                ],
                at: path
            )
            _ = try requireObject(result, key: "scan", at: path)
            _ = try requireString(result, key: "collection_name", at: path)
            try validateDestination(
                try requireObject(result, key: "destination", at: path),
                at: "\(path).destination"
            )
            _ = try requireString(result, key: "destination_path", at: path)
            _ = try requireString(result, key: "accepted_date", at: path)
            _ = try requireObject(result, key: "preflight", at: path)
            _ = try requireBoolean(result, key: "can_start", at: path)
        default:
            throw ValidationError("\(path) has an unsupported result type")
        }
        return type
    }

    private static func validateDestination(
        _ destination: [String: Any],
        at path: String
    ) throws {
        switch try requireString(destination, key: "kind", at: path) {
        case "create":
            try requireKeys(destination, exactly: ["kind", "collection_name"], at: path)
            _ = try requireString(destination, key: "collection_name", at: path)
        case "existing":
            try requireKeys(destination, exactly: ["kind", "identity"], at: path)
            let identity = try requireObject(destination, key: "identity", at: path)
            switch try requireString(identity, key: "kind", at: "\(path).identity") {
            case "dated":
                try requireKeys(
                    identity,
                    exactly: ["kind", "year", "directory"],
                    at: "\(path).identity"
                )
                _ = try requireString(identity, key: "year", at: "\(path).identity")
                _ = try requireString(identity, key: "directory", at: "\(path).identity")
            case "ongoing":
                try requireKeys(
                    identity,
                    exactly: ["kind", "key"],
                    at: "\(path).identity"
                )
                _ = try requireString(identity, key: "key", at: "\(path).identity")
            default:
                throw ValidationError("\(path).identity has an unsupported identity kind")
            }
        default:
            throw ValidationError("\(path) has an unsupported destination kind")
        }
    }

    private static func validateCollection(_ value: Any, at path: String) throws {
        let collection = try requireObject(value, at: path)
        let kind = try requireString(collection, key: "kind", at: path)
        let identity = try requireObject(collection, key: "identity", at: path)
        _ = try requireString(collection, key: "label", at: path)
        switch kind {
        case "dated":
            try requireKeys(
                collection,
                exactly: ["kind", "identity", "label", "start_date", "end_date"],
                at: path
            )
            try requireKeys(identity, exactly: ["kind", "year", "directory"], at: "\(path).identity")
            guard try requireString(identity, key: "kind", at: "\(path).identity") == "dated" else {
                throw ValidationError("\(path).identity must be dated")
            }
            _ = try requireString(identity, key: "year", at: "\(path).identity")
            _ = try requireString(identity, key: "directory", at: "\(path).identity")
            _ = try requireString(collection, key: "start_date", at: path)
            _ = try requireString(collection, key: "end_date", at: path)
        case "ongoing":
            try requireKeys(
                collection,
                exactly: ["kind", "identity", "label", "parts"],
                at: path
            )
            try requireKeys(identity, exactly: ["kind", "key"], at: "\(path).identity")
            guard try requireString(identity, key: "kind", at: "\(path).identity") == "ongoing" else {
                throw ValidationError("\(path).identity must be ongoing")
            }
            _ = try requireString(identity, key: "key", at: "\(path).identity")
            let parts = try requireArray(collection, key: "parts", at: path)
            for (index, value) in parts.enumerated() {
                let partPath = "\(path).parts[\(index)]"
                let part = try requireObject(value, at: partPath)
                try requireKeys(
                    part,
                    exactly: ["year", "year_directory", "collection_directory"],
                    at: partPath
                )
                _ = try requireInteger(part, key: "year", at: partPath)
                _ = try requireString(part, key: "year_directory", at: partPath)
                _ = try requireString(part, key: "collection_directory", at: partPath)
            }
        default:
            throw ValidationError("\(path) has an unsupported collection kind")
        }
    }

    private static func validateBrowseCollection(_ value: Any, at path: String) throws {
        let collection = try requireObject(value, at: path)
        let kind = try requireString(collection, key: "kind", at: path)
        let identity = try requireObject(collection, key: "identity", at: path)
        _ = try requireString(collection, key: "label", at: path)
        _ = try requireInteger(collection, key: "media_count", at: path)
        switch kind {
        case "dated":
            try requireKeys(
                collection,
                exactly: [
                    "kind", "identity", "label", "start_date", "end_date",
                    "media_count", "cover_media", "date_groups", "problem",
                ],
                at: path
            )
            try requireKeys(identity, exactly: ["kind", "year", "directory"], at: "\(path).identity")
            guard try requireString(identity, key: "kind", at: "\(path).identity") == "dated" else {
                throw ValidationError("\(path).identity must be dated")
            }
            _ = try requireString(collection, key: "start_date", at: path)
            _ = try requireString(collection, key: "end_date", at: path)
            try validateDateGroups(
                try requireArray(collection, key: "date_groups", at: path),
                at: "\(path).date_groups"
            )
        case "ongoing":
            try requireKeys(
                collection,
                exactly: ["kind", "identity", "label", "media_count", "parts"],
                at: path
            )
            try requireKeys(identity, exactly: ["kind", "key"], at: "\(path).identity")
            guard try requireString(identity, key: "kind", at: "\(path).identity") == "ongoing" else {
                throw ValidationError("\(path).identity must be ongoing")
            }
            let parts = try requireArray(collection, key: "parts", at: path)
            for (index, value) in parts.enumerated() {
                let partPath = "\(path).parts[\(index)]"
                let part = try requireObject(value, at: partPath)
                try requireKeys(
                    part,
                    exactly: [
                        "year", "year_directory", "collection_directory",
                        "media_count", "cover_media", "date_groups", "problem",
                    ],
                    at: partPath
                )
                _ = try requireInteger(part, key: "year", at: partPath)
                _ = try requireString(part, key: "year_directory", at: partPath)
                _ = try requireString(part, key: "collection_directory", at: partPath)
                _ = try requireInteger(part, key: "media_count", at: partPath)
                try validateDateGroups(
                    try requireArray(part, key: "date_groups", at: partPath),
                    at: "\(partPath).date_groups"
                )
            }
        default:
            throw ValidationError("\(path) has an unsupported browse collection kind")
        }
    }

    private static func validateDateGroups(_ groups: [Any], at path: String) throws {
        for (index, value) in groups.enumerated() {
            let groupPath = "\(path)[\(index)]"
            let group = try requireObject(value, at: groupPath)
            try requireKeys(group, exactly: ["date", "media"], at: groupPath)
            _ = try requireString(group, key: "date", at: groupPath)
            for (mediaIndex, mediaValue) in try requireArray(
                group,
                key: "media",
                at: groupPath
            ).enumerated() {
                let mediaPath = "\(groupPath).media[\(mediaIndex)]"
                let media = try requireObject(mediaValue, at: mediaPath)
                try requireKeys(media, exactly: ["filename", "path"], at: mediaPath)
                _ = try requireString(media, key: "filename", at: mediaPath)
                _ = try requireString(media, key: "path", at: mediaPath)
            }
        }
    }

    private static func validateEvent(
        _ object: [String: Any],
        lineNumber: Int
    ) throws -> ProtocolFixtureShape {
        let path = "line \(lineNumber)"
        let operationID = try requireString(object, key: "operation_id", at: path)
        _ = try requireInteger(object, key: "sequence", at: path)
        let event = try requireString(object, key: "event", at: path)

        switch event {
        case "started":
            try requireKeys(
                object,
                exactly: ["kind", "protocol_version", "operation_id", "sequence", "event", "name"],
                at: path
            )
            _ = try requireString(object, key: "name", at: path)
        case "progress":
            try requireKeys(
                object,
                exactly: [
                    "kind", "protocol_version", "operation_id", "sequence", "event",
                    "completed_units", "total_units", "message",
                ],
                at: path
            )
            _ = try requireInteger(object, key: "completed_units", at: path)
            _ = try requireInteger(object, key: "total_units", at: path)
            _ = try requireString(object, key: "message", at: path)
        case "completed":
            try requireKeys(
                object,
                exactly: ["kind", "protocol_version", "operation_id", "sequence", "event", "result"],
                at: path
            )
            let result = try requireObject(object, key: "result", at: path)
            try requireKeys(result, exactly: ["item_count"], at: "\(path).result")
            _ = try requireInteger(result, key: "item_count", at: "\(path).result")
        case "failed":
            try requireKeys(
                object,
                exactly: ["kind", "protocol_version", "operation_id", "sequence", "event", "error"],
                at: path
            )
            let error = try requireObject(object, key: "error", at: path)
            try requireKeys(error, exactly: ["code", "message"], at: "\(path).error")
            _ = try requireString(error, key: "code", at: "\(path).error")
            _ = try requireString(error, key: "message", at: "\(path).error")
        case "cancelled":
            try requireKeys(
                object,
                exactly: [
                    "kind", "protocol_version", "operation_id", "sequence", "event",
                    "copied_count",
                ],
                at: path
            )
            _ = try requireInteger(object, key: "copied_count", at: path)
        default:
            throw ValidationError("\(path) has an unsupported event type")
        }
        return .event(operationID: operationID, event: event)
    }

    private static func requireKeys(
        _ object: [String: Any],
        exactly expected: Set<String>,
        at path: String
    ) throws {
        let actual = Set(object.keys)
        guard actual == expected else {
            throw ValidationError(
                "\(path) keys were \(actual.sorted()); expected \(expected.sorted())"
            )
        }
    }

    private static func requireObject(
        _ value: Any,
        at path: String
    ) throws -> [String: Any] {
        guard let object = value as? [String: Any] else {
            throw ValidationError("\(path) must be an object")
        }
        return object
    }

    private static func requireObject(
        _ object: [String: Any],
        key: String,
        at path: String
    ) throws -> [String: Any] {
        guard let value = object[key] else {
            throw ValidationError("\(path).\(key) is required")
        }
        return try requireObject(value, at: "\(path).\(key)")
    }

    private static func requireArray(
        _ object: [String: Any],
        key: String,
        at path: String
    ) throws -> [Any] {
        guard let value = object[key] as? [Any] else {
            throw ValidationError("\(path).\(key) must be an array")
        }
        return value
    }

    private static func requireString(
        _ object: [String: Any],
        key: String,
        at path: String
    ) throws -> String {
        guard let value = object[key] else {
            throw ValidationError("\(path).\(key) is required")
        }
        return try requireString(value, at: "\(path).\(key)")
    }

    private static func requireString(
        _ value: Any,
        at path: String
    ) throws -> String {
        guard let string = value as? String else {
            throw ValidationError("\(path) must be a string")
        }
        return string
    }

    @discardableResult
    private static func requireInteger(
        _ object: [String: Any],
        key: String,
        at path: String,
        equals expected: Int? = nil
    ) throws -> Int {
        guard let number = object[key] as? NSNumber,
              CFGetTypeID(number) != CFBooleanGetTypeID(),
              ["c", "s", "i", "l", "q", "C", "S", "I", "L", "Q"]
                .contains(String(cString: number.objCType))
        else {
            throw ValidationError("\(path).\(key) must be an integer")
        }
        let integer = number.intValue
        if let expected, integer != expected {
            throw ValidationError("\(path).\(key) must equal \(expected)")
        }
        return integer
    }

    private static func requireBoolean(
        _ object: [String: Any],
        key: String,
        at path: String
    ) throws -> Bool {
        guard let number = object[key] as? NSNumber,
              CFGetTypeID(number) == CFBooleanGetTypeID()
        else {
            throw ValidationError("\(path).\(key) must be a boolean")
        }
        return number.boolValue
    }

    private struct ValidationError: Error, CustomStringConvertible {
        let description: String

        init(_ description: String) {
            self.description = description
        }
    }
}
