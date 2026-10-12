import AppKit
import Foundation
import XCTest
@testable import Souvenirs

@MainActor
final class SourceDiscoveryTests: XCTestCase {
    func testFiltersUnreadableIneligibleAndCatalogVolumes() async throws {
        let discovery = MountedSourceDiscovery(
            enumerateVolumes: {
                [
                    Self.volume("/Volumes/EOS_DIGITAL", readable: true, removable: true),
                    Self.volume("/Volumes/READ_ONLY", readable: false, removable: true),
                    Self.volume("/Volumes/NAS", readable: true),
                    Self.volume("/Volumes/ARCHIVE", readable: true, ejectable: true)
                ]
            },
            resolveCatalogVolume: { _ in URL(fileURLWithPath: "/Volumes/ARCHIVE") }
        )

        let sources = try await discovery.discoverSources(
            catalogPath: "/Volumes/ARCHIVE/Family Catalog"
        )

        XCTAssertEqual(
            sources,
            [
                ConnectedSource(
                    id: "path:/Volumes/EOS_DIGITAL",
                    rootPath: "/Volumes/EOS_DIGITAL",
                    name: "EOS_DIGITAL"
                )
            ]
        )
    }

    func testUsesStableOrderingAndPathQualifiedUUIDIdentity() async throws {
        let discovery = MountedSourceDiscovery(
            enumerateVolumes: {
                [
                    Self.volume(
                        "/Volumes/Zeta",
                        name: "Card 10",
                        uuid: "duplicate",
                        readable: true,
                        removable: true
                    ),
                    Self.volume(
                        "/Volumes/Alpha",
                        name: "Card 2",
                        uuid: "duplicate",
                        readable: true,
                        ejectable: true
                    ),
                    Self.volume(
                        "/Volumes/Beta",
                        name: "Card 2",
                        uuid: "unique",
                        readable: true,
                        removable: true
                    )
                ]
            },
            resolveCatalogVolume: { _ in nil }
        )

        let sources = try await discovery.discoverSources(catalogPath: "/catalog")

        XCTAssertEqual(sources.map(\.name), ["Card 2", "Card 2", "Card 10"])
        XCTAssertEqual(sources.map(\.rootPath), [
            "/Volumes/Alpha",
            "/Volumes/Beta",
            "/Volumes/Zeta"
        ])
        XCTAssertEqual(sources.map(\.id), [
            "uuid:duplicate|path:/Volumes/Alpha",
            "uuid:unique|path:/Volumes/Beta",
            "uuid:duplicate|path:/Volumes/Zeta"
        ])
    }

    func testObservesMountAndUnmountWithoutInspectingNotificationPayload() {
        let center = NotificationCenter()
        let discovery = MountedSourceDiscovery(
            notificationCenter: center,
            enumerateVolumes: { [] },
            resolveCatalogVolume: { _ in nil }
        )
        var refreshCount = 0
        discovery.startObserving {
            refreshCount += 1
        }

        center.post(name: NSWorkspace.didMountNotification, object: "ignored")
        center.post(name: NSWorkspace.didUnmountNotification, object: "ignored")

        XCTAssertEqual(refreshCount, 2)
        discovery.stopObserving()
        center.post(name: NSWorkspace.didMountNotification, object: nil)
        XCTAssertEqual(refreshCount, 2)
    }

    nonisolated private static func volume(
        _ path: String,
        name: String? = nil,
        uuid: String? = nil,
        readable: Bool? = nil,
        removable: Bool? = nil,
        ejectable: Bool? = nil
    ) -> MountedVolume {
        MountedVolume(
            rootURL: URL(fileURLWithPath: path),
            localizedName: name,
            uuid: uuid,
            isReadable: readable,
            isRemovable: removable,
            isEjectable: ejectable
        )
    }
}
