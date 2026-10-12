import Foundation

@MainActor
protocol CatalogEngineClient: AnyObject {
    func validateCatalog(path: String, intent: String) async throws
        -> EngineSupervisor.CatalogLocation
    func createCatalog(
        path: String,
        displayName: String
    ) async throws -> EngineSupervisor.CatalogLocation
    func openCatalog(path: String) async throws -> EngineSupervisor.CatalogLocation
    func catalogCollections(path: String) async throws -> [EngineSupervisor.CatalogCollection]
    func catalogBrowse(
        path: String,
        selection: EngineSupervisor.CatalogCollectionIdentity?
    ) async throws -> EngineSupervisor.CatalogBrowseSnapshot
    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan
    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        destination: EngineSupervisor.ImportDestination,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview
}

extension EngineAppModel: CatalogEngineClient {
    func validateCatalog(
        path: String,
        intent: String
    ) async throws -> EngineSupervisor.CatalogLocation {
        try await supervisor.validateCatalog(path: path, intent: intent)
    }

    func createCatalog(
        path: String,
        displayName: String
    ) async throws -> EngineSupervisor.CatalogLocation {
        try await supervisor.createCatalog(path: path, displayName: displayName)
    }

    func openCatalog(path: String) async throws -> EngineSupervisor.CatalogLocation {
        try await supervisor.openCatalog(path: path)
    }

    func catalogCollections(
        path: String
    ) async throws -> [EngineSupervisor.CatalogCollection] {
        try await supervisor.catalogCollections(path: path)
    }

    func catalogBrowse(
        path: String,
        selection: EngineSupervisor.CatalogCollectionIdentity?
    ) async throws -> EngineSupervisor.CatalogBrowseSnapshot {
        try await supervisor.catalogBrowse(path: path, selection: selection)
    }

    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan {
        try await supervisor.scanImport(sourcePath: sourcePath)
    }

    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        destination: EngineSupervisor.ImportDestination,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview {
        try await supervisor.reviewImport(
            catalogPath: catalogPath,
            sourcePath: sourcePath,
            destination: destination,
            dateResolution: dateResolution
        )
    }
}

struct RecentCatalog: Codable, Equatable, Identifiable {
    let path: String
    let name: String
    let volumeName: String

    var id: String { path }
}

@MainActor
final class CatalogWelcomeModel: ObservableObject {
    enum Route: Equatable {
        case welcome
        case create
        case catalog(RecentCatalog)
    }

    enum Validation: Equatable {
        case none
        case checking
        case valid(volumeName: String)
        case invalid(String)
    }

    @Published private(set) var route: Route = .welcome
    @Published var catalogName = "Family Archive"
    @Published private(set) var selectedPath: String?
    @Published private(set) var validation: Validation = .none
    @Published private(set) var isWorking = false
    @Published private(set) var recents: [RecentCatalog]
    @Published private(set) var importPlanning: ImportPlanningModel?
    @Published var presentedError: String?
    @Published private(set) var presentedErrorTitle = "Souvenirs couldn’t open this archive"
    let browse: CatalogBrowseModel

    private let engine: CatalogEngineClient
    private let defaults: UserDefaults
    private static let recentsKey = "recentCatalogs"

    init(
        engine: CatalogEngineClient,
        defaults: UserDefaults = .standard,
        recents: [RecentCatalog]? = nil
    ) {
        self.engine = engine
        browse = CatalogBrowseModel(engine: engine)
        self.defaults = defaults
        if let recents {
            self.recents = recents
        } else if let data = defaults.data(forKey: Self.recentsKey),
                  let decoded = try? JSONDecoder().decode([RecentCatalog].self, from: data) {
            self.recents = decoded
        } else {
            self.recents = []
        }
    }

    var canCreate: Bool {
        !catalogName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && selectedPath != nil
            && {
                if case .valid = validation { return true }
                return false
            }()
            && !isWorking
    }

    func beginCreate() {
        route = .create
        selectedPath = nil
        validation = .none
        presentedError = nil
    }

    func cancelCreate() {
        route = .welcome
        selectedPath = nil
        validation = .none
        presentedError = nil
    }

    func chooseCreateLocation(_ path: String) async {
        selectedPath = path
        validation = .checking
        do {
            let location = try await engine.validateCatalog(path: path, intent: "create")
            guard selectedPath == path else { return }
            validation = .valid(volumeName: location.volumeName)
        } catch {
            guard selectedPath == path else { return }
            validation = .invalid(userMessage(for: error))
        }
    }

    func createCatalog() async {
        guard canCreate, let selectedPath else { return }
        isWorking = true
        defer { isWorking = false }
        do {
            let location = try await engine.createCatalog(
                path: selectedPath,
                displayName: catalogName
            )
            showCatalog(location, name: catalogName)
        } catch {
            presentedErrorTitle = "Souvenirs couldn’t create this archive"
            presentedError = userMessage(for: error)
        }
    }

    func openCatalog(at path: String, name: String? = nil) async {
        isWorking = true
        defer { isWorking = false }
        do {
            let location = try await engine.openCatalog(path: path)
            showCatalog(
                location,
                name: name
                    ?? location.displayName
                    ?? URL(fileURLWithPath: path).lastPathComponent
            )
        } catch {
            presentedErrorTitle = "Souvenirs couldn’t open this archive"
            presentedError = userMessage(for: error)
        }
    }

    func removeRecent(path: String) {
        recents.removeAll { $0.path == path }
        persistRecents()
    }

    func beginImport(from sourcePath: String, into catalog: RecentCatalog) {
        let planning = ImportPlanningModel(
            engine: engine,
            catalog: catalog,
            sourcePath: sourcePath
        )
        importPlanning = planning
        Task { await planning.load() }
    }

    func dismissImport() {
        importPlanning = nil
    }

    private func showCatalog(
        _ location: EngineSupervisor.CatalogLocation,
        name: String
    ) {
        let recent = RecentCatalog(
            path: location.path,
            name: name.trimmingCharacters(in: .whitespacesAndNewlines),
            volumeName: location.volumeName
        )
        recents.removeAll { $0.path == recent.path }
        recents.insert(recent, at: 0)
        recents = Array(recents.prefix(8))
        persistRecents()
        route = .catalog(recent)
    }

    private func persistRecents() {
        if let data = try? JSONEncoder().encode(recents) {
            defaults.set(data, forKey: Self.recentsKey)
        }
    }

    private func userMessage(for error: Error) -> String {
        if case EngineSupervisor.SupervisorError.protocolError(let code, let message) = error {
            return archiveUserMessage(code: code, message: message)
        }
        return error.localizedDescription
    }
}

@MainActor
final class ImportPlanningModel: ObservableObject {
    @Published private(set) var scan: EngineSupervisor.ImportScan?
    @Published private(set) var review: EngineSupervisor.ImportReview?
    @Published var collectionName = ""
    @Published private(set) var destinationMode: EngineSupervisor.ImportDestinationMode = .create
    @Published private(set) var selectedExistingCollection:
        EngineSupervisor.CatalogCollection?
    @Published private(set) var availableCollections: [EngineSupervisor.CatalogCollection] = []
    @Published private(set) var isLoadingCollections = false
    @Published private(set) var collectionSelectionError: String?
    @Published private(set) var dateResolution: EngineSupervisor.ImportDateResolution?
    @Published private(set) var dateChoiceSources:
        [EngineSupervisor.ImportDateSourceSummary] = []
    @Published private(set) var isLoading = false
    @Published private(set) var errorCode: String?
    @Published private(set) var errorMessage: String?
    @Published var showsDetails = false
    @Published private(set) var isPrepared = false

    let catalog: RecentCatalog
    let sourcePath: String
    @Published private(set) var acceptedDate = ""
    private let engine: CatalogEngineClient
    private var reviewGeneration = 0
    private var createCollectionName = ""

    init(
        engine: CatalogEngineClient,
        catalog: RecentCatalog,
        sourcePath: String
    ) {
        self.engine = engine
        self.catalog = catalog
        self.sourcePath = sourcePath
    }

    var canStart: Bool {
        guard let review else { return false }
        return review.canStart
            && review.reviewedPlan != nil
            && review.collectionName == collectionName
            && review.destination == destination
            && review.acceptedDate == acceptedDate
            && review.dateResolution == dateResolution
            && !isLoading
            && errorMessage == nil
    }

    var dateChoiceCount: Int {
        interpretationChoiceSources.reduce(0) { $0 + $1.itemCount }
    }

    var deferredDateCount: Int {
        dateChoiceSources
            .filter(\.requiresClock)
            .reduce(0) { $0 + $1.itemCount }
    }

    var interpretationChoiceSources: [EngineSupervisor.ImportDateSourceSummary] {
        dateChoiceSources.filter {
            !$0.requiresClock
        }
    }

    var supportsUTCConversion: Bool {
        interpretationChoiceSources.contains { $0.supportsUTCConversion }
    }

    var isStartBlocked: Bool {
        if errorMessage != nil { return true }
        guard let review, !isLoading else { return false }
        return !review.canStart || review.reviewedPlan == nil
    }

    var canRetryScan: Bool {
        scan == nil
            && errorCode == "source_unavailable"
            && errorMessage != nil
            && !isLoading
    }

    var dateChoicePreventsStart: Bool {
        errorMessage == nil && review?.canStart == false
    }

    var startImportAccessibilityHint: String {
        if isPrepared {
            return "The import plan is ready."
        }
        if let errorMessage {
            return "Import cannot start. \(errorMessage)"
        }
        if !canStart {
            return "Import cannot start until the review is ready."
        }
        return "Starts the reviewed import."
    }

    var utcOffsetDescription: String {
        let seconds = TimeZone.current.secondsFromGMT()
        let sign = seconds < 0 ? "−" : "+"
        let magnitude = abs(seconds)
        return String(
            format: "UTC%@%02d:%02d",
            sign,
            magnitude / 3_600,
            (magnitude % 3_600) / 60
        )
    }

    var title: String {
        if scan == nil, errorCode == "source_unavailable" {
            return "This source is unavailable"
        }
        guard let scan else { return "Reading this source…" }
        return "\(review?.transferCount ?? scan.mediaCount) new photos and videos will be copied into \(destinationDescription)"
    }

    var destinationDescription: String {
        if collectionName.isEmpty { return "a new collection" }
        return "“\(collectionName)”"
    }

    var destination: EngineSupervisor.ImportDestination {
        if let selectedExistingCollection {
            return .existing(identity: selectedExistingCollection.identity)
        }
        return .create(collectionName: collectionName)
    }

    func load() async {
        guard !isLoading else { return }
        scan = nil
        review = nil
        isPrepared = false
        isLoading = true
        errorCode = nil
        errorMessage = nil
        do {
            let scan = try await engine.scanImport(sourcePath: sourcePath)
            self.scan = scan
            dateChoiceSources = scan.datetimeSources.filter {
                $0.state == .needsConfirmation
            }
            collectionName = scan.suggestedCollectionName
            createCollectionName = collectionName
            acceptedDate = scan.suggestedAcceptedDate ?? ""
            await refreshReview()
        } catch {
            isLoading = false
            errorCode = protocolErrorCode(for: error)
            errorMessage = userMessage(for: error)
        }
    }

    func retryScan() async {
        guard canRetryScan else { return }
        await load()
    }

    func refreshReview() async {
        guard scan != nil else { return }
        reviewGeneration += 1
        let generation = reviewGeneration
        let destination = destination
        let dateResolution = dateResolution
        isLoading = true
        errorCode = nil
        errorMessage = nil
        isPrepared = false
        do {
            let result = try await engine.reviewImport(
                catalogPath: catalog.path,
                sourcePath: sourcePath,
                destination: destination,
                dateResolution: dateResolution
            )
            guard generation == reviewGeneration else { return }
            scan = result.scan
            self.acceptedDate = result.acceptedDate
            review = result
        } catch {
            guard generation == reviewGeneration else { return }
            review = nil
            errorCode = protocolErrorCode(for: error)
            errorMessage = userMessage(for: error)
        }
        if generation == reviewGeneration {
            isLoading = false
        }
    }

    func collectionNameChanged() {
        guard destinationMode == .create else { return }
        createCollectionName = collectionName
        reviewGeneration += 1
        review = nil
        errorCode = nil
        errorMessage = nil
        isLoading = false
        isPrepared = false
    }

    func loadCollections() async {
        guard !isLoadingCollections else { return }
        isLoadingCollections = true
        collectionSelectionError = nil
        do {
            availableCollections = try await engine.catalogCollections(path: catalog.path)
        } catch {
            collectionSelectionError = userMessage(for: error)
        }
        isLoadingCollections = false
    }

    func selectExistingCollection(_ collection: EngineSupervisor.CatalogCollection) async {
        selectedExistingCollection = collection
        destinationMode = .existing
        collectionName = collection.label
        await refreshReview()
    }

    func useCreateDestination() async {
        selectedExistingCollection = nil
        destinationMode = .create
        collectionName = createCollectionName
        await refreshReview()
    }

    func resolveDate(_ resolution: EngineSupervisor.ImportDateResolution) async {
        dateResolution = resolution
        await refreshReview()
    }

    func changeDateResolution() async {
        dateResolution = nil
        await refreshReview()
    }

    func prepareExecution() {
        guard canStart else { return }
        isPrepared = true
    }

    private func userMessage(for error: Error) -> String {
        if case EngineSupervisor.SupervisorError.protocolError(let code, let message) = error {
            return archiveUserMessage(code: code, message: message)
        }
        return error.localizedDescription
    }

    private func protocolErrorCode(for error: Error) -> String? {
        guard case EngineSupervisor.SupervisorError.protocolError(let code, _) = error else {
            return nil
        }
        return code
    }
}

func archiveUserMessage(code: String, message: String) -> String {
    switch code {
    case "catalog_already_exists":
        return "This folder is already a Souvenirs archive."
    case "catalog_configuration_mismatch":
        return "This archive was created with different settings."
    case "unsupported_catalog_version":
        return "This archive uses a format that this version of Souvenirs cannot open."
    case "catalog_io":
        return """
            Souvenirs couldn’t access this folder. Check that it’s available and allows changes, \
            then try again.
            """
    case "invalid_catalog":
        return "This archive is damaged or unreadable."
    case "invalid_operation_journal":
        return "Souvenirs couldn’t read this archive’s recovery information."
    case "not_a_catalog":
        return "This folder is not a Souvenirs archive. Choose a folder containing .souvenirs.yml."
    case "synchronized_folder":
        return """
            This synchronized folder cannot hold an archive because it may rename files on its own.
            """
    case "catalog_error":
        return "Souvenirs couldn’t access this archive. Check its folder or volume, then try again."
    case "operation_conflict" where message.contains("catalog"):
        return "The archive changed after review. Review the import again."
    case "operation_failed" where message.contains("catalog"):
        return "The import couldn’t update this archive. Check its folder or volume, then try again."
    default:
        if message.contains("catalog") {
            return """
                Souvenirs couldn’t complete this archive operation. Check the selected folders and \
                volumes, then try again.
                """
        }
        return message
    }
}
