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
