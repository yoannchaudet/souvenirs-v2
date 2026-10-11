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
    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan
    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        collectionName: String,
        destinationMode: EngineSupervisor.ImportDestinationMode,
        acceptedDate: String,
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

    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan {
        try await supervisor.scanImport(sourcePath: sourcePath)
    }

    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        collectionName: String,
        destinationMode: EngineSupervisor.ImportDestinationMode,
        acceptedDate: String,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview {
        try await supervisor.reviewImport(
            catalogPath: catalogPath,
            sourcePath: sourcePath,
            collectionName: collectionName,
            destinationMode: destinationMode,
            acceptedDate: acceptedDate,
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
    @Published var catalogName = "Family Catalog"
    @Published private(set) var selectedPath: String?
    @Published private(set) var validation: Validation = .none
    @Published private(set) var isWorking = false
    @Published private(set) var recents: [RecentCatalog]
    @Published private(set) var importPlanning: ImportPlanningModel?
    @Published var presentedError: String?
    @Published private(set) var presentedErrorTitle = "Souvenirs couldn’t open this catalog"

    private let engine: CatalogEngineClient
    private let defaults: UserDefaults
    private static let recentsKey = "recentCatalogs"

    init(
        engine: CatalogEngineClient,
        defaults: UserDefaults = .standard,
        recents: [RecentCatalog]? = nil
    ) {
        self.engine = engine
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
            presentedErrorTitle = "Souvenirs couldn’t create this catalog"
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
            presentedErrorTitle = "Souvenirs couldn’t open this catalog"
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
        if case EngineSupervisor.SupervisorError.protocolError(_, let message) = error {
            return message
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
    @Published private(set) var dateResolution: EngineSupervisor.ImportDateResolution?
    @Published private(set) var dateChoiceSources:
        [EngineSupervisor.ImportDateSourceSummary] = []
    @Published private(set) var isLoading = false
    @Published private(set) var errorMessage: String?
    @Published var showsDetails = false
    @Published private(set) var isPrepared = false

    let catalog: RecentCatalog
    let sourcePath: String
    @Published private(set) var acceptedDate = ""
    private let engine: CatalogEngineClient
    private var reviewGeneration = 0

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
            && review.destinationMode == destinationMode
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

    var asShownTime: String? {
        dateChoiceSources.lazy.compactMap(\.asShownTime).first
    }

    var convertedFromUTCTime: String? {
        dateChoiceSources.lazy.compactMap(\.convertedFromUTCTime).first
    }

    var isStartBlocked: Bool {
        if errorMessage != nil { return true }
        guard let review, !isLoading else { return false }
        return !review.canStart || review.reviewedPlan == nil
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
        guard let scan else { return "Reading this source…" }
        return "\(review?.transferCount ?? scan.mediaCount) new photos and videos will be copied into \(destinationDescription)"
    }

    var destinationDescription: String {
        if collectionName.isEmpty { return "a new collection" }
        return "“\(collectionName)”"
    }

    func load() async {
        isLoading = true
        errorMessage = nil
        do {
            let scan = try await engine.scanImport(sourcePath: sourcePath)
            self.scan = scan
            dateChoiceSources = scan.datetimeSources.filter {
                $0.state == .needsConfirmation
            }
            collectionName = scan.suggestedCollectionName
            acceptedDate = scan.suggestedAcceptedDate ?? ""
            await refreshReview()
        } catch {
            isLoading = false
            errorMessage = userMessage(for: error)
        }
    }

    func refreshReview() async {
        guard scan != nil else { return }
        reviewGeneration += 1
        let generation = reviewGeneration
        let collectionName = collectionName
        let destinationMode = destinationMode
        let acceptedDate = acceptedDate
        let dateResolution = dateResolution
        isLoading = true
        errorMessage = nil
        isPrepared = false
        do {
            let result = try await engine.reviewImport(
                catalogPath: catalog.path,
                sourcePath: sourcePath,
                collectionName: collectionName,
                destinationMode: destinationMode,
                acceptedDate: acceptedDate,
                dateResolution: dateResolution
            )
            guard generation == reviewGeneration else { return }
            scan = result.scan
            self.acceptedDate = result.acceptedDate
            review = result
        } catch {
            guard generation == reviewGeneration else { return }
            review = nil
            errorMessage = userMessage(for: error)
        }
        if generation == reviewGeneration {
            isLoading = false
        }
    }

    func collectionNameChanged() {
        reviewGeneration += 1
        review = nil
        errorMessage = nil
        isLoading = false
        isPrepared = false
    }

    func setDestinationMode(_ mode: EngineSupervisor.ImportDestinationMode) async {
        destinationMode = mode
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
        if case EngineSupervisor.SupervisorError.protocolError(_, let message) = error {
            return message
        }
        return error.localizedDescription
    }
}
