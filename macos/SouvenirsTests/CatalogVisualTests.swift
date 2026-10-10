import AppKit
import SwiftUI
import XCTest
@testable import Souvenirs

@MainActor
final class CatalogVisualTests: XCTestCase {
    func testCaptureCatalogFlowScreens() async throws {
        let engine = VisualCatalogEngine()
        let model = CatalogWelcomeModel(
            engine: engine,
            defaults: UserDefaults(suiteName: "CatalogVisualTests")!,
            recents: [
                RecentCatalog(
                    path: "/Volumes/photos/Family Catalog",
                    name: "Family Catalog",
                    volumeName: "photos-nas.local"
                )
            ]
        )

        try capture(
            ContentView(engineStatus: .ready(engineVersion: "0.1.0"), model: model),
            size: NSSize(width: 586, height: 386),
            name: "catalog-welcome"
        )

        model.beginCreate()
        await model.chooseCreateLocation("/Volumes/photos/Family Catalog")
        try capture(
            ContentView(engineStatus: .ready(engineVersion: "0.1.0"), model: model),
            size: NSSize(width: 465, height: 455),
            name: "catalog-create"
        )

        await model.createCatalog()
        try capture(
            ContentView(engineStatus: .ready(engineVersion: "0.1.0"), model: model),
            size: NSSize(width: 920, height: 620),
            name: "catalog-ready"
        )

        if case .catalog(let catalog) = model.route {
            model.beginImport(from: "/Volumes/EOS_DIGITAL", into: catalog)
            for _ in 0..<10 where model.importPlanning?.review == nil {
                await Task.yield()
            }
            try capture(
                ContentView(engineStatus: .ready(engineVersion: "0.1.0"), model: model),
                size: NSSize(width: 920, height: 640),
                name: "import-review"
            )
        }
    }

    private func capture<ViewType: View>(
        _ view: ViewType,
        size: NSSize,
        name: String
    ) throws {
        let hostingView = NSHostingView(rootView: view)
        hostingView.frame = NSRect(origin: .zero, size: size)
        hostingView.layoutSubtreeIfNeeded()
        guard let representation = hostingView.bitmapImageRepForCachingDisplay(
            in: hostingView.bounds
        ) else {
            return XCTFail("Could not create a bitmap for \(name)")
        }
        hostingView.cacheDisplay(in: hostingView.bounds, to: representation)
        guard let data = representation.representation(using: .png, properties: [:]) else {
            return XCTFail("Could not encode \(name)")
        }

        let directory = URL(fileURLWithPath: "/tmp/SouvenirsScreenshots")
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        let url = directory.appendingPathComponent("\(name).png")
        try data.write(to: url)

        let attachment = XCTAttachment(data: data, uniformTypeIdentifier: "public.png")
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}

@MainActor
private final class VisualCatalogEngine: CatalogEngineClient {
    func validateCatalog(path: String, intent: String) async throws
        -> EngineSupervisor.CatalogLocation {
        location(path)
    }

    func createCatalog(
        path: String,
        displayName: String
    ) async throws -> EngineSupervisor.CatalogLocation {
        location(path)
    }

    func openCatalog(path: String) async throws -> EngineSupervisor.CatalogLocation {
        location(path)
    }

    func scanImport(sourcePath: String) async throws -> EngineSupervisor.ImportScan {
        .init(
            sourcePath: sourcePath,
            sourceName: "EOS_DIGITAL",
            fileCount: 17,
            mediaCount: 12,
            photoCount: 8,
            videoCount: 4,
            unsupportedCount: 2,
            uncertainTimeCount: 4,
            totalBytes: 41_000_000_000,
            suggestedCollectionName: "Dolomites",
            suggestedAcceptedDate: "2026-10-10"
        )
    }

    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        collectionName: String,
        destinationMode: EngineSupervisor.ImportDestinationMode,
        acceptedDate: String,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview {
        let scan = try await scanImport(sourcePath: sourcePath)
        return .init(
            scan: scan,
            collectionName: collectionName,
            destinationMode: destinationMode,
            destinationPath: "\(catalogPath)/2026/\(acceptedDate) \(collectionName)",
            acceptedDate: acceptedDate,
            dateResolution: dateResolution,
            requiredDecisionCount: dateResolution == nil ? 1 : 0,
            transferCount: 12,
            skippedCount: 0,
            leftOnSourceCount: 2,
            operationCount: 14,
            requiredSpaceBytes: 41_000_000_000,
            canStart: dateResolution != nil
        )
    }

    private func location(_ path: String) -> EngineSupervisor.CatalogLocation {
        .init(
            path: path,
            volumeName: "photos-nas.local",
            displayName: "Family Catalog",
            detail: "Catalog format 2"
        )
    }
}
