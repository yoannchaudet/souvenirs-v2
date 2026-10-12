import AppKit
import ImageIO
import SwiftUI

struct CatalogBrowseSidebar: View {
    @ObservedObject var model: CatalogBrowseModel
    @FocusState private var listFocused: Bool

    var body: some View {
        List(selection: selection) {
            ForEach(Array(model.navigation.enumerated()), id: \.offset) { _, scope in
                switch scope {
                case .allPhotos:
                    Label("All Photos", systemImage: "square.grid.2x2")
                        .tag(CatalogBrowseModel.Scope.allPhotos)
                        .accessibilityIdentifier("archive-all-photos")
                case .year(let year, let entries):
                    VStack(alignment: .leading, spacing: 1) {
                        Text(String(year))
                        Text(collectionCount(entries.count))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    .tag(CatalogBrowseModel.Scope.year(year))
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("archive-year-\(year)")
                    .accessibilityLabel(String(year))
                    .accessibilityValue(collectionCount(entries.count))
                case .earlier(let throughYear, let years):
                    VStack(alignment: .leading, spacing: 1) {
                        Text("Earlier")
                        Text(earlierRange(years))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    .tag(CatalogBrowseModel.Scope.earlier(throughYear: throughYear))
                    .accessibilityElement(children: .combine)
                    .accessibilityIdentifier("archive-earlier")
                    .accessibilityLabel("Earlier")
                    .accessibilityValue(earlierRange(years))
                }
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
        .disabled(model.blocksBrowseNavigation)
        .focused($listFocused)
        .defaultFocus($listFocused, true)
    }

    private var selection: Binding<CatalogBrowseModel.Scope?> {
        Binding(
            get: {
                guard case .overview = model.selection else { return nil }
                return model.selectedScope
            },
            set: { value in
                guard let value else { return }
                Task { @MainActor in
                    model.selectScope(value)
                }
            }
        )
    }

    private func collectionCount(_ count: Int) -> String {
        "\(count) \(count == 1 ? "collection" : "collections")"
    }

    private func earlierRange(
        _ years: [EngineSupervisor.CatalogBrowseNavigationYear]
    ) -> String {
        guard let newest = years.first?.year, let oldest = years.last?.year else {
            return "No earlier years"
        }
        return newest == oldest ? String(newest) : "\(oldest)–\(newest)"
    }
}

struct CatalogBrowseHeader: View {
    @ObservedObject var model: CatalogBrowseModel
    @FocusState private var titleFocused: Bool
    @AccessibilityFocusState private var titleAccessibilityFocused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(model.displayTitle)
                .font(.headline)
                .focusable(model.importReconciliation == .landed)
                .focused($titleFocused)
                .accessibilityFocused($titleAccessibilityFocused)
                .accessibilityAddTraits(.isHeader)
                .accessibilityIdentifier("archive-browse-title")
            if let subtitle = model.displaySubtitle {
                Text(subtitle)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("archive-browse-subtitle")
            }
        }
        .onChange(of: model.landingFocusGeneration) { oldValue, newValue in
            guard newValue > oldValue else { return }
            titleFocused = true
            titleAccessibilityFocused = true
        }
    }
}

struct CatalogBrowsePane<EmptyContent: View>: View {
    @ObservedObject var model: CatalogBrowseModel
    let emptyContent: () -> EmptyContent

    var body: some View {
        Group {
            if let collection = model.displayedCollection {
                CatalogCollectionDetail(collection: collection)
            } else if let errorMessage = model.errorMessage {
                browseError(errorMessage)
            } else if model.isLoadingOverview && model.collections.isEmpty {
                ProgressView("Reading archive…")
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .accessibilityIdentifier("archive-browse-loading")
            } else if model.isEmpty {
                emptyContent()
            } else {
                switch model.selection {
                case .overview:
                    CatalogOverview(model: model)
                case .collection:
                    if model.isLoadingDetail {
                        ProgressView("Reading collection…")
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .accessibilityIdentifier("archive-detail-loading")
                    } else if let detail = model.displayedCollection {
                        CatalogCollectionDetail(collection: detail)
                    } else {
                        ProgressView("Refreshing archive…")
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                }
            }
        }
    }

    private func browseError(_ message: String) -> some View {
        ContentUnavailableView {
            Label("Archive Unavailable", systemImage: "externaldrive.badge.exclamationmark")
        } description: {
            Text(message)
        } actions: {
            Button("Try Again") {
                Task { await model.refresh() }
            }
            .accessibilityIdentifier("archive-browse-retry")
        }
        .accessibilityIdentifier("archive-browse-error")
    }

}

private struct CatalogOverview: View {
    @ObservedObject var model: CatalogBrowseModel

    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 18) {
                ForEach(Array(model.overviewGroups.enumerated()), id: \.element.id) {
                    groupIndex,
                    group in
                    if let year = group.year {
                        Text(String(year))
                            .font(.title3.bold())
                            .accessibilityAddTraits(.isHeader)
                            .accessibilityIdentifier("archive-overview-year-\(year)")
                    }
                    LazyVGrid(
                        columns: [
                            GridItem(.adaptive(minimum: 170, maximum: 240), spacing: 14)
                        ],
                        spacing: 14
                    ) {
                        ForEach(Array(group.entries.enumerated()), id: \.element) {
                            itemIndex,
                            entry in
                            if let collection = model.collection(for: entry) {
                                collectionButton(
                                    collection,
                                    entry: entry,
                                    identifier: "\(groupIndex)-\(itemIndex)"
                                )
                            }
                        }
                    }
                }
            }
            .padding(18)
        }
        .accessibilityIdentifier("archive-overview")
    }

    private func collectionButton(
        _ collection: EngineSupervisor.CatalogBrowseCollection,
        entry: EngineSupervisor.CatalogBrowseNavigationEntry,
        identifier: String
    ) -> some View {
        Button {
            Task {
                await model.select(
                    .init(
                        identity: entry.identity,
                        annualPartYear: entry.annualPartYear
                    )
                )
            }
        } label: {
            collectionCard(collection, annualPartYear: entry.annualPartYear)
        }
        .buttonStyle(.plain)
        .disabled(model.blocksBrowseNavigation)
        .accessibilityIdentifier("archive-cover-\(identifier)")
        .accessibilityLabel(collection.browseLabel)
        .accessibilityValue(
            accessibilityValue(collection, annualPartYear: entry.annualPartYear)
        )
    }

    private func collectionCard(
        _ collection: EngineSupervisor.CatalogBrowseCollection,
        annualPartYear: Int?
    ) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ArchiveThumbnail(
                media: collection.trustedCoverMedia(annualPartYear: annualPartYear),
                height: 116
            )
            Text(collection.browseLabel)
                .font(.headline)
                .lineLimit(1)
            Text(collection.browseSubtitle(annualPartYear: annualPartYear))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
            if let count = collection.trustedMediaCount(annualPartYear: annualPartYear) {
                Text("\(count) \(count == 1 ? "item" : "items")")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                Label("Some content unavailable", systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.background.opacity(0.78), in: RoundedRectangle(cornerRadius: 12))
        .overlay {
            RoundedRectangle(cornerRadius: 12)
                .stroke(.separator.opacity(0.45))
        }
    }

    private func accessibilityValue(
        _ collection: EngineSupervisor.CatalogBrowseCollection,
        annualPartYear: Int?
    ) -> String {
        if let count = collection.trustedMediaCount(annualPartYear: annualPartYear) {
            return "\(collection.browseSubtitle(annualPartYear: annualPartYear)), \(count) "
                + (count == 1 ? "item" : "items")
        }
        let subtitle = collection.browseSubtitle(annualPartYear: annualPartYear)
        return "\(subtitle), some content unavailable"
    }
}

private struct CatalogCollectionDetail: View {
    let collection: CatalogBrowseModel.PresentedCollection

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                switch collection {
                case .dated(_, _, _, _, _, _, let groups, let problem):
                    if let problem {
                        CatalogProblemNotice(
                            title: "Collection unavailable",
                            problem: problem,
                            identifier: "archive-problem-dated"
                        )
                    } else if groups.isEmpty {
                        emptyCollection
                    } else {
                        dateGroups(groups, identifierPrefix: "dated")
                    }
                case .ongoing(_, _, _, let parts):
                    if parts.isEmpty {
                        emptyCollection
                    } else {
                        ForEach(Array(parts.enumerated()), id: \.offset) { _, part in
                            annualPart(part)
                        }
                    }
                }
            }
            .padding(18)
        }
        .accessibilityIdentifier("archive-collection-detail")
    }

    @ViewBuilder
    private func annualPart(_ part: CatalogBrowseModel.PresentedPart) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(String(part.year))
                .font(.title3.bold())
                .accessibilityAddTraits(.isHeader)
            if let problem = part.problem {
                CatalogProblemNotice(
                    title: "\(part.year) unavailable",
                    problem: problem,
                    identifier: "archive-problem-part-\(part.year)"
                )
            } else if part.dateGroups.isEmpty {
                emptyCollection
            } else {
                dateGroups(
                    part.dateGroups,
                    identifierPrefix: "part-\(part.year)"
                )
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("archive-part-\(part.year)")
    }

    private func dateGroups(
        _ groups: [CatalogBrowseModel.PresentedDateGroup],
        identifierPrefix: String
    ) -> some View {
        ForEach(Array(groups.enumerated()), id: \.offset) { groupIndex, group in
            VStack(alignment: .leading, spacing: 8) {
                Text(group.date)
                    .font(.headline)
                    .accessibilityAddTraits(.isHeader)
                    .accessibilityIdentifier(
                        "archive-\(identifierPrefix)-date-group-\(groupIndex)"
                    )
                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: 128, maximum: 180), spacing: 10)],
                    spacing: 10
                ) {
                    ForEach(Array(group.media.enumerated()), id: \.offset) {
                        mediaIndex,
                        media in
                        VStack(alignment: .leading, spacing: 5) {
                            ArchiveThumbnail(media: media, height: 100)
                            Text(media.filename)
                                .font(.caption)
                                .lineLimit(2)
                                .multilineTextAlignment(.leading)
                        }
                        .focusable()
                        .accessibilityIdentifier(
                            "archive-\(identifierPrefix)-media-\(groupIndex)-\(mediaIndex)"
                        )
                        .accessibilityLabel(media.filename)
                        .accessibilityValue(
                            media.isTransient
                                ? "Pending archive confirmation, accepted on \(group.date)"
                                : "Archived on \(group.date)"
                        )
                    }
                }
            }
            .accessibilityElement(children: .contain)
        }
    }

    private var emptyCollection: some View {
        ContentUnavailableView(
            "No Archived Originals",
            systemImage: "photo.on.rectangle.angled",
            description: Text("This collection does not contain admitted originals yet.")
        )
        .frame(maxWidth: .infinity)
        .accessibilityIdentifier("archive-collection-empty")
    }
}

private struct CatalogProblemNotice: View {
    let title: String
    let problem: String
    let identifier: String

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).fontWeight(.semibold)
                Text(problem)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        } icon: {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier(identifier)
    }
}

private struct ArchiveThumbnail: View {
    let path: String?
    let isTransient: Bool
    let height: CGFloat
    @State private var image: NSImage?
    @State private var didFail = false

    init(media: EngineSupervisor.CatalogBrowseMedia?, height: CGFloat) {
        path = media?.path
        isTransient = false
        self.height = height
    }

    init(media: CatalogBrowseModel.PresentedMedia?, height: CGFloat) {
        path = media?.path
        isTransient = media?.isTransient ?? false
        self.height = height
    }

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 8)
                .fill(.quaternary.opacity(0.65))
            if let image {
                Image(nsImage: image)
                    .resizable()
                    .scaledToFill()
            } else if didFail || path == nil {
                Image(systemName: "photo")
                    .font(.title2)
                    .foregroundStyle(.secondary)
            } else {
                ProgressView()
                    .controlSize(.small)
            }
        }
        .overlay(alignment: .topTrailing) {
            if isTransient {
                Image(systemName: "clock.badge.checkmark")
                    .padding(6)
                    .background(.regularMaterial, in: Circle())
                    .padding(6)
            }
        }
        .frame(maxWidth: .infinity)
        .frame(height: height)
        .clipShape(RoundedRectangle(cornerRadius: 8))
        .accessibilityHidden(true)
        .task(id: path) {
            image = nil
            didFail = false
            guard let path else {
                didFail = true
                return
            }
            let loaded = await ArchiveThumbnailLoader.shared.load(path: path)
            guard !Task.isCancelled else { return }
            image = loaded.map { NSImage(cgImage: $0, size: .zero) }
            didFail = loaded == nil
        }
    }
}

private actor ArchiveThumbnailLoader {
    static let shared = ArchiveThumbnailLoader()

    func load(path: String) -> CGImage? {
        guard !Task.isCancelled else { return nil }
        let url = URL(fileURLWithPath: path) as CFURL
        guard let source = CGImageSourceCreateWithURL(url, nil),
              !Task.isCancelled else {
            return nil
        }
        let options = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: 480
        ] as CFDictionary
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options)
    }
}
