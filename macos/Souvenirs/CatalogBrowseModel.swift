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

    struct PresentedMedia: Equatable, Hashable {
        let filename: String
        let path: String
        let isTransient: Bool
    }

    struct PresentedDateGroup: Equatable {
        let date: String
        let media: [PresentedMedia]
    }

    struct PresentedPart: Equatable {
        let year: Int
        let mediaCount: Int?
        let coverMedia: PresentedMedia?
        let dateGroups: [PresentedDateGroup]
        let problem: String?
    }

    enum PresentedCollection: Equatable {
        case dated(
            identity: EngineSupervisor.CatalogCollectionIdentity,
            label: String,
            startDate: String,
            endDate: String,
            mediaCount: Int?,
            coverMedia: PresentedMedia?,
            dateGroups: [PresentedDateGroup],
            problem: String?
        )
        case ongoing(
            identity: EngineSupervisor.CatalogCollectionIdentity,
            label: String,
            mediaCount: Int?,
            parts: [PresentedPart]
        )

        var identity: EngineSupervisor.CatalogCollectionIdentity {
            switch self {
            case .dated(let identity, _, _, _, _, _, _, _),
                 .ongoing(let identity, _, _, _):
                return identity
            }
        }

        var label: String {
            switch self {
            case .dated(_, let label, _, _, _, _, _, _),
                 .ongoing(_, let label, _, _):
                return label
            }
        }
    }

    enum ImportReconciliation: Equatable {
        case active
        case refreshing
        case failed(String)
        case landed
    }

    private struct TransientImport: Equatable {
        let operationID: String
        let collectionName: String
        let media: [EngineSupervisor.OperationMedia]
        let occurrence: CollectionOccurrence?
        let isConsistent: Bool
        var reconciliation: ImportReconciliation
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
    @Published private(set) var landingFocusGeneration = 0

    private let engine: CatalogEngineClient
    private var catalogPath: String?
    private var generation = 0
    private var transientImport: TransientImport?
    private var importGeneration = 0

    init(engine: CatalogEngineClient) {
        self.engine = engine
    }

    var isEmpty: Bool {
        !isLoadingOverview
            && errorMessage == nil
            && collections.isEmpty
            && presentedImportCollection == nil
    }

    var importReconciliation: ImportReconciliation? {
        transientImport?.reconciliation
    }

    var blocksBrowseNavigation: Bool {
        transientImport?.reconciliation != nil
            && transientImport?.reconciliation != .landed
    }

    var presentedImportCollection: PresentedCollection? {
        guard let transientImport,
              transientImport.reconciliation != .landed,
              let occurrence = transientImport.occurrence else {
            return nil
        }
        let durable = detail?.identity == occurrence.identity
            ? detail
            : collections.first(where: { $0.identity == occurrence.identity })
        return Self.merge(
            durable: durable,
            transient: transientImport.media,
            collectionName: transientImport.collectionName
        )
    }

    var displayedCollection: PresentedCollection? {
        if let presentedImportCollection {
            return presentedImportCollection
        }
        return detail.map(Self.presented)
    }

    var displayTitle: String {
        if let displayedCollection {
            return displayedCollection.label
        }
        if let transientImport, transientImport.reconciliation != .landed {
            return transientImport.collectionName.isEmpty
                ? "Importing" : transientImport.collectionName
        }
        guard case .collection(let occurrence) = selection else {
            switch selectedScope {
            case .allPhotos:
                return "All Photos"
            case .year(let year):
                return String(year)
            case .earlier:
                return "Earlier"
            }
        }
        return collections.first(where: {
            $0.identity == occurrence.identity
        })?.browseLabel ?? "Collection"
    }

    var displaySubtitle: String? {
        if let displayedCollection {
            let subtitle = displayedCollection.subtitle
            if let count = displayedCollection.mediaCount {
                return "\(subtitle) · \(count) \(count == 1 ? "item" : "items")"
            }
            return "\(subtitle) · Some content unavailable"
        }
        guard presentedImportCollection == nil else { return nil }
        switch selection {
        case .overview:
            guard !isLoadingOverview, errorMessage == nil else { return nil }
            switch selectedScope {
            case .allPhotos:
                return collectionCount(collections.count)
            case .year:
                return collectionCount(overviewGroups.first?.entries.count ?? 0)
            case .earlier:
                let groups = overviewGroups
                guard let newest = groups.first?.year, let oldest = groups.last?.year else {
                    return collectionCount(0)
                }
                let count = groups.reduce(0) { $0 + $1.entries.count }
                return "\(oldest)–\(newest) · \(collectionCount(count))"
            }
        case .collection:
            return nil
        }
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
        case .earlier:
            guard let scope = navigation.first(where: {
                if case .earlier = $0 { return true }
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

    func updateImportPresentation(
        operationID: String?,
        collectionName: String,
        media: [EngineSupervisor.OperationMedia],
        loadDetail: Bool = true
    ) async {
        guard let operationID else {
            clearImportPresentation()
            return
        }
        let occurrence = media.first.map {
            CollectionOccurrence(
                identity: $0.identity,
                annualPartYear: $0.annualPartYear
            )
        }
        guard media.allSatisfy({ record in
            Self.isValid(record)
                && occurrence.map {
                    record.identity == $0.identity
                } != false
        }) else {
            transientImport = TransientImport(
                operationID: operationID,
                collectionName: collectionName,
                media: [],
                occurrence: nil,
                isConsistent: false,
                reconciliation: .failed("The engine returned inconsistent verified media.")
            )
            return
        }

        let isNewOperation = transientImport?.operationID != operationID
        let previousOccurrence = isNewOperation ? nil : transientImport?.occurrence
        let reconciliation = isNewOperation
            ? ImportReconciliation.active
            : transientImport?.reconciliation ?? .active
        transientImport = TransientImport(
            operationID: operationID,
            collectionName: collectionName,
            media: media,
            occurrence: occurrence,
            isConsistent: true,
            reconciliation: reconciliation
        )
        guard reconciliation != .landed, let occurrence else { return }
        if isNewOperation || previousOccurrence != occurrence {
            selection = .collection(occurrence)
            selectedScope = scopeContaining(occurrence, in: navigation) ?? selectedScope
        }

        guard loadDetail, previousOccurrence != occurrence else {
            return
        }
        await loadImportDetail(occurrence, operationID: operationID)
    }

    func reconcileTerminalImport() async {
        guard let catalogPath, var transientImport else { return }
        guard transientImport.isConsistent,
              transientImport.reconciliation != .refreshing,
              transientImport.reconciliation != .landed else {
            return
        }
        importGeneration += 1
        let requestGeneration = importGeneration
        let operationID = transientImport.operationID
        transientImport.reconciliation = .refreshing
        self.transientImport = transientImport

        do {
            let overview = try await engine.catalogBrowse(path: catalogPath, selection: nil)
            guard requestGeneration == importGeneration,
                  self.transientImport?.operationID == operationID else {
                return
            }
            guard let occurrence = transientImport.occurrence else {
                commitTerminalOverview(overview)
                return
            }
            let scoped = try await engine.catalogBrowse(
                path: catalogPath,
                selection: occurrence.identity
            )
            guard requestGeneration == importGeneration,
                  self.transientImport?.operationID == operationID else {
                return
            }
            guard overviewContains(occurrence, snapshot: overview),
                  let selected = scoped.collections.first(where: {
                      $0.identity == occurrence.identity
                  }),
                  Self.containsAll(
                      transientImport.media,
                      in: selected
                  ) else {
                throw TerminalRefreshError.destinationUnavailable
            }

            generation += 1
            navigation = overview.navigation
            collections = overview.collections
            selectedScope = scopeContaining(occurrence, in: overview.navigation)
                ?? .allPhotos
            selection = .collection(occurrence)
            detail = selected
            isLoadingOverview = false
            isLoadingDetail = false
            errorMessage = nil
            self.transientImport?.reconciliation = .landed
            landingFocusGeneration += 1
        } catch is CancellationError {
            guard self.transientImport?.operationID == operationID else { return }
            self.transientImport?.reconciliation = .active
        } catch {
            guard requestGeneration == importGeneration,
                  self.transientImport?.operationID == operationID else {
                return
            }
            self.transientImport?.reconciliation = .failed(
                terminalRefreshMessage(for: error)
            )
        }
    }

    func clearImportPresentation() {
        guard transientImport != nil else { return }
        importGeneration += 1
        transientImport = nil
    }

    func selectScope(_ scope: Scope) {
        guard scopeExists(scope) else { return }
        if !isLoadingOverview {
            generation += 1
        }
        selectedScope = resolvedScope(scope) ?? scope
        selection = .overview
        detail = nil
        isLoadingDetail = false
    }

    private func loadImportDetail(
        _ occurrence: CollectionOccurrence,
        operationID: String
    ) async {
        guard let catalogPath else { return }
        importGeneration += 1
        let requestGeneration = importGeneration
        do {
            let result = try await engine.catalogBrowse(
                path: catalogPath,
                selection: occurrence.identity
            )
            guard requestGeneration == importGeneration,
                  transientImport?.operationID == operationID,
                  transientImport?.occurrence == occurrence,
                  transientImport?.reconciliation != .landed else {
                return
            }
            detail = result.collections.first(where: {
                $0.identity == occurrence.identity
            })
        } catch is CancellationError {
            return
        } catch {
            guard requestGeneration == importGeneration,
                  transientImport?.operationID == operationID else {
                return
            }
            transientImport?.reconciliation = .failed(userMessage(for: error))
        }
    }

    private func commitTerminalOverview(
        _ overview: EngineSupervisor.CatalogBrowseSnapshot
    ) {
        generation += 1
        navigation = overview.navigation
        collections = overview.collections
        selectedScope = .allPhotos
        selection = .overview
        detail = nil
        isLoadingOverview = false
        isLoadingDetail = false
        errorMessage = nil
        transientImport?.reconciliation = .landed
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
            let liveScope = keepingSnapshot ? selectedScope : scope
            let liveSelection = selection
            selectedScope = resolvedScope(liveScope) ?? .allPhotos
            selection = .overview
            detail = nil
            guard let occurrence,
                  !keepingSnapshot || liveSelection == .collection(occurrence),
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
        resolvedScope(scope) != nil
    }

    private func resolvedScope(_ scope: Scope) -> Scope? {
        switch scope {
        case .allPhotos:
            return navigation.contains(where: {
                if case .allPhotos = $0 { return true }
                return false
            }) ? .allPhotos : nil
        case .year(let selectedYear):
            return navigation.contains(where: {
                if case .year(let year, _) = $0 {
                    return year == selectedYear
                }
                return false
            }) ? scope : nil
        case .earlier:
            guard let navigationScope = navigation.first(where: {
                if case .earlier = $0 { return true }
                return false
            }), case .earlier(let throughYear, _) = navigationScope else { return nil }
            return .earlier(throughYear: throughYear)
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

    private func scopeContaining(
        _ occurrence: CollectionOccurrence,
        in navigation: [EngineSupervisor.CatalogBrowseNavigationScope]
    ) -> Scope? {
        for scope in navigation {
            switch scope {
            case .allPhotos:
                continue
            case .year(let year, let entries):
                if entries.contains(where: { entry in
                    entry.identity == occurrence.identity
                        && entry.annualPartYear == occurrence.annualPartYear
                }) {
                    return .year(year)
                }
            case .earlier(let throughYear, let years):
                if years.contains(where: { year in
                    year.entries.contains(where: { entry in
                        entry.identity == occurrence.identity
                            && entry.annualPartYear == occurrence.annualPartYear
                    })
                }) {
                    return .earlier(throughYear: throughYear)
                }
            }
        }
        return navigation.contains(where: {
            if case .allPhotos = $0 { return true }
            return false
        }) ? .allPhotos : nil
    }

    private func overviewContains(
        _ occurrence: CollectionOccurrence,
        snapshot: EngineSupervisor.CatalogBrowseSnapshot
    ) -> Bool {
        snapshot.collections.contains(where: { $0.identity == occurrence.identity })
            && scopeContaining(occurrence, in: snapshot.navigation) != nil
    }

    private func terminalRefreshMessage(for error: Error) -> String {
        if case TerminalRefreshError.destinationUnavailable = error {
            return "Souvenirs couldn’t confirm the imported media in the archive. Check that the archive is available, then try again."
        }
        return userMessage(for: error)
    }

    private func collectionCount(_ count: Int) -> String {
        "\(count) \(count == 1 ? "collection" : "collections")"
    }

    private static func isValid(_ media: EngineSupervisor.OperationMedia) -> Bool {
        switch media.identity {
        case .dated:
            return media.annualPartYear == nil
        case .ongoing:
            return media.annualPartYear != nil
        }
    }

    private static func presented(
        _ collection: EngineSupervisor.CatalogBrowseCollection
    ) -> PresentedCollection {
        switch collection {
        case .dated(
            let identity,
            let label,
            let startDate,
            let endDate,
            let mediaCount,
            let coverMedia,
            let dateGroups,
            let problem
        ):
            return .dated(
                identity: identity,
                label: label,
                startDate: startDate,
                endDate: endDate,
                mediaCount: problem == nil ? mediaCount : nil,
                coverMedia: problem == nil ? coverMedia.map(presented) : nil,
                dateGroups: problem == nil ? dateGroups.map(presented) : [],
                problem: problem
            )
        case .ongoing(let identity, let label, let mediaCount, let parts):
            return .ongoing(
                identity: identity,
                label: label,
                mediaCount: parts.contains(where: { $0.problem != nil }) ? nil : mediaCount,
                parts: parts.map { part in
                    PresentedPart(
                        year: part.year,
                        mediaCount: part.problem == nil ? part.mediaCount : nil,
                        coverMedia: part.problem == nil
                            ? part.coverMedia.map(presented) : nil,
                        dateGroups: part.problem == nil
                            ? part.dateGroups.map(presented) : [],
                        problem: part.problem
                    )
                }
            )
        }
    }

    private static func presented(
        _ group: EngineSupervisor.CatalogBrowseDateGroup
    ) -> PresentedDateGroup {
        PresentedDateGroup(
            date: group.date,
            media: group.media.map(presented)
        )
    }

    private static func presented(
        _ media: EngineSupervisor.CatalogBrowseMedia
    ) -> PresentedMedia {
        PresentedMedia(
            filename: media.filename,
            path: media.path,
            isTransient: false
        )
    }

    private static func presented(
        _ media: EngineSupervisor.OperationMedia
    ) -> PresentedMedia {
        PresentedMedia(
            filename: media.filename,
            path: media.path,
            isTransient: true
        )
    }

    private static func merge(
        durable: EngineSupervisor.CatalogBrowseCollection?,
        transient: [EngineSupervisor.OperationMedia],
        collectionName: String
    ) -> PresentedCollection? {
        guard let first = transient.first else {
            return durable.map(presented)
        }
        let durablePresentation = durable.map(presented)
        switch first.identity {
        case .dated:
            let durableGroups: [PresentedDateGroup]
            let label: String
            let startDate: String
            let endDate: String
            let problem: String?
            if case .dated(
                _,
                let durableLabel,
                let durableStart,
                let durableEnd,
                _,
                _,
                let groups,
                let durableProblem
            ) = durablePresentation {
                durableGroups = groups
                label = durableLabel
                startDate = durableStart
                endDate = durableEnd
                problem = durableProblem
            } else {
                durableGroups = []
                label = collectionName
                startDate = transient.map(\.acceptedDate).min() ?? ""
                endDate = transient.map(\.acceptedDate).max() ?? startDate
                problem = nil
            }
            let groups = mergeGroups(durable: durableGroups, transient: transient)
            let media = groups.flatMap(\.media)
            return .dated(
                identity: first.identity,
                label: label,
                startDate: startDate,
                endDate: endDate,
                mediaCount: problem == nil ? media.count : nil,
                coverMedia: problem == nil ? media.first : nil,
                dateGroups: problem == nil ? groups : [],
                problem: problem
            )
        case .ongoing:
            let durableParts: [PresentedPart]
            let label: String
            if case .ongoing(_, let durableLabel, _, let parts) = durablePresentation {
                durableParts = parts
                label = durableLabel
            } else {
                durableParts = []
                label = collectionName
            }
            let transientYears = orderedUnique(
                transient.compactMap(\.annualPartYear)
            )
            var mergedParts = durableParts
            for year in transientYears {
                let records = transient.filter { $0.annualPartYear == year }
                if let index = mergedParts.firstIndex(where: { $0.year == year }) {
                    let part = mergedParts[index]
                    let groups = mergeGroups(
                        durable: part.dateGroups,
                        transient: records
                    )
                    let media = groups.flatMap(\.media)
                    mergedParts[index] = PresentedPart(
                        year: year,
                        mediaCount: part.problem == nil ? media.count : nil,
                        coverMedia: part.problem == nil ? media.first : nil,
                        dateGroups: part.problem == nil ? groups : [],
                        problem: part.problem
                    )
                } else {
                    let groups = mergeGroups(durable: [], transient: records)
                    let media = groups.flatMap(\.media)
                    mergedParts.append(
                        PresentedPart(
                            year: year,
                            mediaCount: media.count,
                            coverMedia: media.first,
                            dateGroups: groups,
                            problem: nil
                        )
                    )
                }
            }
            let count = mergedParts.contains(where: { $0.problem != nil })
                ? nil
                : mergedParts.compactMap(\.mediaCount).reduce(0, +)
            return .ongoing(
                identity: first.identity,
                label: label,
                mediaCount: count,
                parts: mergedParts
            )
        }
    }

    private static func mergeGroups(
        durable: [PresentedDateGroup],
        transient: [EngineSupervisor.OperationMedia]
    ) -> [PresentedDateGroup] {
        var groups = durable
        var durablePaths = Set(durable.flatMap(\.media).map(\.path))
        for record in transient where durablePaths.insert(record.path).inserted {
            if let index = groups.firstIndex(where: { $0.date == record.acceptedDate }) {
                let group = groups[index]
                groups[index] = PresentedDateGroup(
                    date: group.date,
                    media: group.media + [presented(record)]
                )
            } else {
                groups.append(
                    PresentedDateGroup(
                        date: record.acceptedDate,
                        media: [presented(record)]
                    )
                )
            }
        }
        return groups
    }

    private static func orderedUnique<T: Hashable>(_ values: [T]) -> [T] {
        var seen: Set<T> = []
        return values.filter { seen.insert($0).inserted }
    }

    private static func containsAll(
        _ transient: [EngineSupervisor.OperationMedia],
        in collection: EngineSupervisor.CatalogBrowseCollection
    ) -> Bool {
        let durablePaths: Set<String>
        switch collection {
        case .dated(_, _, _, _, _, _, let groups, let problem):
            guard problem == nil else { return false }
            durablePaths = Set(groups.flatMap(\.media).map(\.path))
        case .ongoing(_, _, _, let parts):
            guard parts.allSatisfy({ $0.problem == nil }) else { return false }
            durablePaths = Set(
                parts.flatMap(\.dateGroups).flatMap(\.media).map(\.path)
            )
        }
        return transient.allSatisfy { durablePaths.contains($0.path) }
    }

    private enum TerminalRefreshError: Error {
        case destinationUnavailable
    }
}

extension CatalogBrowseModel.PresentedCollection {
    var subtitle: String {
        switch self {
        case .dated(_, _, let startDate, let endDate, _, _, _, _):
            return startDate == endDate ? startDate : "\(startDate) through \(endDate)"
        case .ongoing(_, _, _, let parts):
            return parts.map(\.year).map(String.init).joined(separator: ", ")
        }
    }

    var mediaCount: Int? {
        switch self {
        case .dated(_, _, _, _, let count, _, _, _),
             .ongoing(_, _, let count, _):
            return count
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
