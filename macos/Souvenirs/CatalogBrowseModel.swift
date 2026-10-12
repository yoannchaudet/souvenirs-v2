import Foundation

@MainActor
final class CatalogBrowseModel: ObservableObject {
    enum Scope: Hashable {
        case allPhotos
        case year(Int)
        case earlier(throughYear: Int)
    }

    struct CollectionOccurrence: Hashable {
        let identity: EngineSupervisor.CatalogCollectionIdentity
        let annualPartYear: Int?
    }

    enum Selection: Hashable {
        case overview
        case collection(CollectionOccurrence)
    }

    struct OverviewGroup: Identifiable {
        let year: Int?
        let entries: [EngineSupervisor.CatalogBrowseNavigationEntry]

        var id: String {
            year.map(String.init) ?? "all"
        }
    }

    @Published private(set) var navigation:
        [EngineSupervisor.CatalogBrowseNavigationScope] = []
    @Published private(set) var collections:
        [EngineSupervisor.CatalogBrowseCollection] = []
    @Published private(set) var selectedScope: Scope = .allPhotos
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

    var overviewGroups: [OverviewGroup] {
        switch selectedScope {
        case .allPhotos:
            return [
                OverviewGroup(
                    year: nil,
                    entries: collections.map {
                        .init(identity: $0.identity, annualPartYear: nil)
                    }
                )
            ]
        case .year(let selectedYear):
            guard let scope = navigation.first(where: {
                if case .year(let year, _) = $0 {
                    return year == selectedYear
                }
                return false
            }), case .year(let year, let entries) = scope else {
                return []
            }
            return [OverviewGroup(year: year, entries: entries)]
        case .earlier(let selectedThroughYear):
            guard let scope = navigation.first(where: {
                if case .earlier(let throughYear, _) = $0 {
                    return throughYear == selectedThroughYear
                }
                return false
            }), case .earlier(_, let years) = scope else {
                return []
            }
            return years.map { OverviewGroup(year: $0.year, entries: $0.entries) }
        }
    }

    func load(catalogPath: String) async {
        guard self.catalogPath != catalogPath || collections.isEmpty || errorMessage != nil
        else { return }
        self.catalogPath = catalogPath
        await reload(keepingSnapshot: false)
    }

    func refresh() async {
        let preservedOccurrence: CollectionOccurrence?
        if case .collection(let occurrence) = selection {
            preservedOccurrence = occurrence
        } else {
            preservedOccurrence = nil
        }
        await reload(
            preservingScope: selectedScope,
            preservingOccurrence: preservedOccurrence,
            keepingSnapshot: true
        )
    }

    func selectScope(_ scope: Scope) {
        guard !isLoadingOverview, scopeExists(scope) else { return }
        generation += 1
        selectedScope = scope
        selection = .overview
        detail = nil
        isLoadingDetail = false
    }

    func select(_ occurrence: CollectionOccurrence) async {
        guard !isLoadingOverview else { return }
        guard selection != .collection(occurrence) || detail == nil else { return }
        generation += 1
        let requestGeneration = generation
        selection = .collection(occurrence)
        detail = nil
        isLoadingDetail = false

        guard let catalogPath else { return }

        isLoadingDetail = true
        do {
            let result = try await engine.catalogBrowse(
                path: catalogPath,
                selection: occurrence.identity
            )
            guard requestGeneration == generation else { return }
            isLoadingDetail = false
            guard let selected = result.collections.first(where: {
                $0.identity == occurrence.identity
            }) else {
                await reload(
                    preservingScope: selectedScope,
                    preservingOccurrence: nil
                )
                return
            }
            detail = selected
        } catch {
            fail(error, generation: requestGeneration)
        }
    }

    private func reload(
        preservingScope scope: Scope = .allPhotos,
        preservingOccurrence occurrence: CollectionOccurrence? = nil,
        keepingSnapshot: Bool = false
    ) async {
        guard let catalogPath else { return }
        generation += 1
        let requestGeneration = generation
        errorMessage = nil
        isLoadingDetail = false
        isLoadingOverview = true
        if !keepingSnapshot {
            navigation = []
            collections = []
            detail = nil
            selectedScope = .allPhotos
            selection = .overview
        }

        do {
            let result = try await engine.catalogBrowse(path: catalogPath, selection: nil)
            guard requestGeneration == generation else { return }
            navigation = result.navigation
            collections = result.collections
            isLoadingOverview = false
            selectedScope = scopeExists(scope) ? scope : .allPhotos
            selection = .overview
            detail = nil
            guard let occurrence,
                  occurrenceExists(occurrence),
                  result.collections.contains(where: {
                      $0.identity == occurrence.identity
                  }) else {
                return
            }
            await select(occurrence)
        } catch {
            fail(error, generation: requestGeneration)
        }
    }

    private func fail(_ error: Error, generation requestGeneration: Int) {
        guard requestGeneration == generation else { return }
        navigation = []
        collections = []
        detail = nil
        selectedScope = .allPhotos
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

    func collection(
        for entry: EngineSupervisor.CatalogBrowseNavigationEntry
    ) -> EngineSupervisor.CatalogBrowseCollection? {
        collections.first(where: { $0.identity == entry.identity })
    }

    private func scopeExists(_ scope: Scope) -> Bool {
        switch scope {
        case .allPhotos:
            return navigation.contains(where: {
                if case .allPhotos = $0 { return true }
                return false
            })
        case .year(let selectedYear):
            return navigation.contains(where: {
                if case .year(let year, _) = $0 {
                    return year == selectedYear
                }
                return false
            })
        case .earlier(let selectedThroughYear):
            return navigation.contains(where: {
                if case .earlier(let throughYear, _) = $0 {
                    return throughYear == selectedThroughYear
                }
                return false
            })
        }
    }

    private func occurrenceExists(_ occurrence: CollectionOccurrence) -> Bool {
        navigation.contains { scope in
            switch scope {
            case .allPhotos:
                return occurrence.annualPartYear == nil
                    && collections.contains(where: { $0.identity == occurrence.identity })
            case .year(_, let entries):
                return entries.contains(where: {
                    $0.identity == occurrence.identity
                        && $0.annualPartYear == occurrence.annualPartYear
                })
            case .earlier(_, let years):
                return years.contains(where: { year in
                    year.entries.contains(where: {
                        $0.identity == occurrence.identity
                            && $0.annualPartYear == occurrence.annualPartYear
                    })
                })
            }
        }
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
        browseSubtitle(annualPartYear: nil)
    }

    func browseSubtitle(annualPartYear: Int?) -> String {
        switch self {
        case .dated(_, _, let startDate, let endDate, _, _, _, _):
            return startDate == endDate ? startDate : "\(startDate) through \(endDate)"
        case .ongoing(_, _, _, let parts):
            if let annualPartYear {
                return "\(annualPartYear) annual part"
            }
            return parts.map(\.year).map(String.init).joined(separator: ", ")
        }
    }

    var trustedMediaCount: Int? {
        trustedMediaCount(annualPartYear: nil)
    }

    func trustedMediaCount(annualPartYear: Int?) -> Int? {
        switch self {
        case .dated(_, _, _, _, let count, _, _, let problem):
            return problem == nil ? count : nil
        case .ongoing(_, _, let count, let parts):
            if let annualPartYear {
                guard let part = parts.first(where: { $0.year == annualPartYear }) else {
                    return nil
                }
                return part.problem == nil ? part.mediaCount : nil
            }
            return parts.contains(where: { $0.problem != nil }) ? nil : count
        }
    }

    var trustedCoverMedia: EngineSupervisor.CatalogBrowseMedia? {
        trustedCoverMedia(annualPartYear: nil)
    }

    func trustedCoverMedia(
        annualPartYear: Int?
    ) -> EngineSupervisor.CatalogBrowseMedia? {
        switch self {
        case .dated(_, _, _, _, _, let cover, _, let problem):
            return problem == nil ? cover : nil
        case .ongoing(_, _, _, let parts):
            if let annualPartYear {
                guard let part = parts.first(where: { $0.year == annualPartYear }),
                      part.problem == nil else {
                    return nil
                }
                return part.coverMedia
            }
            return parts.first(where: { $0.problem == nil && $0.coverMedia != nil })?.coverMedia
        }
    }

    var browseProblem: String? {
        browseProblem(annualPartYear: nil)
    }

    func browseProblem(annualPartYear: Int?) -> String? {
        switch self {
        case .dated(_, _, _, _, _, _, _, let problem):
            return problem
        case .ongoing(_, _, _, let parts):
            if let annualPartYear {
                return parts.first(where: { $0.year == annualPartYear })?.problem
            }
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
