import Foundation

@MainActor
final class CatalogBrowseModel: ObservableObject {
    enum Selection: Hashable {
        case overview
        case collection(EngineSupervisor.CatalogCollectionIdentity)
    }

    @Published private(set) var collections:
        [EngineSupervisor.CatalogBrowseCollection] = []
    @Published private(set) var selection: Selection = .overview
    @Published private(set) var detail: EngineSupervisor.CatalogBrowseCollection?
    @Published private(set) var isLoadingOverview = false
    @Published private(set) var isLoadingDetail = false
    @Published private(set) var errorMessage: String?

    private let engine: CatalogEngineClient
    private var catalogPath: String?
    private var generation = 0

    init(engine: CatalogEngineClient) {
        self.engine = engine
    }

    var isEmpty: Bool {
        !isLoadingOverview && errorMessage == nil && collections.isEmpty
    }

    func load(catalogPath: String) async {
        guard self.catalogPath != catalogPath || collections.isEmpty || errorMessage != nil
        else { return }
        self.catalogPath = catalogPath
        await reload(preserving: nil)
    }

    func refresh() async {
        let preservedIdentity: EngineSupervisor.CatalogCollectionIdentity?
        if case .collection(let identity) = selection {
            preservedIdentity = identity
        } else {
            preservedIdentity = nil
        }
        await reload(preserving: preservedIdentity)
    }

    func select(_ newSelection: Selection) async {
        guard !isLoadingOverview else { return }
        guard selection != newSelection || detail == nil else { return }
        generation += 1
        let requestGeneration = generation
        selection = newSelection
        detail = nil
        isLoadingDetail = false

        guard case .collection(let identity) = newSelection,
              let catalogPath else {
            return
        }

        isLoadingDetail = true
        do {
            let result = try await engine.catalogBrowse(
                path: catalogPath,
                selection: identity
            )
            guard requestGeneration == generation else { return }
            isLoadingDetail = false
            guard let selected = result.first(where: { $0.identity == identity }) else {
                await reload(preserving: nil)
                return
            }
            detail = selected
        } catch {
            fail(error, generation: requestGeneration)
        }
    }

    private func reload(
        preserving identity: EngineSupervisor.CatalogCollectionIdentity?
    ) async {
        guard let catalogPath else { return }
        generation += 1
        let requestGeneration = generation
        collections = []
        detail = nil
        selection = .overview
        errorMessage = nil
        isLoadingDetail = false
        isLoadingOverview = true

        do {
            let result = try await engine.catalogBrowse(path: catalogPath, selection: nil)
            guard requestGeneration == generation else { return }
            collections = result
            isLoadingOverview = false

            guard let identity,
                  result.contains(where: { $0.identity == identity }) else {
                return
            }
            await select(.collection(identity))
        } catch {
            fail(error, generation: requestGeneration)
        }
    }

    private func fail(_ error: Error, generation requestGeneration: Int) {
        guard requestGeneration == generation else { return }
        collections = []
        detail = nil
        selection = .overview
        isLoadingOverview = false
        isLoadingDetail = false
        errorMessage = userMessage(for: error)
    }

    private func userMessage(for error: Error) -> String {
        if case EngineSupervisor.SupervisorError.protocolError(let code, let message) = error {
            return archiveUserMessage(code: code, message: message)
        }
        return error.localizedDescription
    }
}

extension EngineSupervisor.CatalogBrowseCollection {
    var browseLabel: String {
        switch self {
        case .dated(_, let label, _, _, _, _, _, _),
             .ongoing(_, let label, _, _):
            return label
        }
    }

    var browseSubtitle: String {
        switch self {
        case .dated(_, _, let startDate, let endDate, _, _, _, _):
            return startDate == endDate ? startDate : "\(startDate) through \(endDate)"
        case .ongoing(_, _, _, let parts):
            return parts.map(\.year).map(String.init).joined(separator: ", ")
        }
    }

    var trustedMediaCount: Int? {
        switch self {
        case .dated(_, _, _, _, let count, _, _, let problem):
            return problem == nil ? count : nil
        case .ongoing(_, _, let count, let parts):
            return parts.contains(where: { $0.problem != nil }) ? nil : count
        }
    }

    var trustedCoverMedia: EngineSupervisor.CatalogBrowseMedia? {
        switch self {
        case .dated(_, _, _, _, _, let cover, _, let problem):
            return problem == nil ? cover : nil
        case .ongoing(_, _, _, let parts):
            return parts.first(where: { $0.problem == nil && $0.coverMedia != nil })?.coverMedia
        }
    }

    var browseProblem: String? {
        switch self {
        case .dated(_, _, _, _, _, _, _, let problem):
            return problem
        case .ongoing(_, _, _, let parts):
            return parts.first(where: { $0.problem != nil })?.problem
        }
    }

    var overviewOnly: Self {
        switch self {
        case .dated(
            let identity,
            let label,
            let startDate,
            let endDate,
            let mediaCount,
            let coverMedia,
            _,
            let problem
        ):
            return .dated(
                identity: identity,
                label: label,
                startDate: startDate,
                endDate: endDate,
                mediaCount: mediaCount,
                coverMedia: coverMedia,
                dateGroups: [],
                problem: problem
            )
        case .ongoing(let identity, let label, let mediaCount, let parts):
            return .ongoing(
                identity: identity,
                label: label,
                mediaCount: mediaCount,
                parts: parts.map {
                    .init(
                        year: $0.year,
                        yearDirectory: $0.yearDirectory,
                        collectionDirectory: $0.collectionDirectory,
                        mediaCount: $0.mediaCount,
                        coverMedia: $0.coverMedia,
                        dateGroups: [],
                        problem: $0.problem
                    )
                }
            )
        }
    }
}
