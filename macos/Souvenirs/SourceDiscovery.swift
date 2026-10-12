import AppKit
import Foundation

struct ConnectedSource: Equatable, Identifiable, Sendable {
    let id: String
    let rootPath: String
    let name: String
}

struct MountedVolume: Equatable, Sendable {
    let rootURL: URL
    let localizedName: String?
    let uuid: String?
    let isReadable: Bool?
    let isRemovable: Bool?
    let isEjectable: Bool?
}

enum SourceDiscoveryError: LocalizedError {
    case enumerationUnavailable

    var errorDescription: String? {
        switch self {
        case .enumerationUnavailable:
            "Souvenirs couldn’t read the connected volumes."
        }
    }
}

@MainActor
protocol SourceDiscoveryClient: AnyObject {
    func discoverSources(catalogPath: String) async throws -> [ConnectedSource]
    func startObserving(_ onChange: @escaping @MainActor () -> Void)
    func stopObserving()
}

@MainActor
final class MountedSourceDiscovery: SourceDiscoveryClient {
    typealias VolumeEnumerator = @Sendable () throws -> [MountedVolume]
    typealias CatalogVolumeResolver = @Sendable (String) -> URL?

    private let notificationCenter: NotificationCenter
    private let enumerateVolumes: VolumeEnumerator
    private let resolveCatalogVolume: CatalogVolumeResolver
    private var observers: [NSObjectProtocol] = []

    init(
        notificationCenter: NotificationCenter = NSWorkspace.shared.notificationCenter,
        enumerateVolumes: @escaping VolumeEnumerator = {
            try MountedSourceDiscovery.systemVolumes()
        },
        resolveCatalogVolume: @escaping CatalogVolumeResolver =
            { MountedSourceDiscovery.systemCatalogVolume($0) }
    ) {
        self.notificationCenter = notificationCenter
        self.enumerateVolumes = enumerateVolumes
        self.resolveCatalogVolume = resolveCatalogVolume
    }

    deinit {
        for observer in observers {
            notificationCenter.removeObserver(observer)
        }
    }

    func discoverSources(catalogPath: String) async throws -> [ConnectedSource] {
        let enumerateVolumes = enumerateVolumes
        let resolveCatalogVolume = resolveCatalogVolume
        let discovered = try await Task.detached {
            (
                try enumerateVolumes(),
                resolveCatalogVolume(catalogPath)
            )
        }.value
        let catalogURL = URL(fileURLWithPath: catalogPath).standardizedFileURL
        let catalogVolume = discovered.1?.standardizedFileURL
        let eligible = discovered.0.filter { volume in
            guard volume.isReadable == true,
                  volume.isRemovable == true || volume.isEjectable == true else {
                return false
            }
            let root = volume.rootURL.standardizedFileURL
            if let catalogVolume, root == catalogVolume {
                return false
            }
            return !Self.isDescendantOrEqual(catalogURL, of: root)
        }

        return eligible.map { volume in
            let root = volume.rootURL.standardizedFileURL
            let name = volume.localizedName?.trimmingCharacters(
                in: .whitespacesAndNewlines
            )
            let displayName = name.flatMap { $0.isEmpty ? nil : $0 }
                ?? root.lastPathComponent
            return ConnectedSource(
                id: Self.identity(for: volume),
                rootPath: root.path,
                name: displayName
            )
        }
        .sorted(by: Self.sourceOrder)
    }

    func startObserving(_ onChange: @escaping @MainActor () -> Void) {
        stopObserving()
        for name in [NSWorkspace.didMountNotification, NSWorkspace.didUnmountNotification] {
            observers.append(
                notificationCenter.addObserver(
                    forName: name,
                    object: nil,
                    queue: .main
                ) { _ in
                    MainActor.assumeIsolated {
                        onChange()
                    }
                }
            )
        }
    }

    func stopObserving() {
        for observer in observers {
            notificationCenter.removeObserver(observer)
        }
        observers.removeAll()
    }

    private static func identity(for volume: MountedVolume) -> String {
        let path = volume.rootURL.standardizedFileURL.path
        if let uuid = volume.uuid?.trimmingCharacters(in: .whitespacesAndNewlines),
           !uuid.isEmpty {
            return "uuid:\(uuid)|path:\(path)"
        }
        return "path:\(path)"
    }

    private static func sourceOrder(_ lhs: ConnectedSource, _ rhs: ConnectedSource) -> Bool {
        let nameOrder = lhs.name.localizedStandardCompare(rhs.name)
        if nameOrder != .orderedSame {
            return nameOrder == .orderedAscending
        }
        if lhs.rootPath != rhs.rootPath {
            return lhs.rootPath < rhs.rootPath
        }
        return lhs.id < rhs.id
    }

    private static func isDescendantOrEqual(_ candidate: URL, of root: URL) -> Bool {
        let candidateComponents = candidate.standardizedFileURL.pathComponents
        let rootComponents = root.standardizedFileURL.pathComponents
        guard candidateComponents.count >= rootComponents.count else {
            return false
        }
        return Array(candidateComponents.prefix(rootComponents.count)) == rootComponents
    }

    nonisolated private static func systemVolumes() throws -> [MountedVolume] {
        let keys: Set<URLResourceKey> = [
            .isReadableKey,
            .volumeIsRemovableKey,
            .volumeIsEjectableKey,
            .volumeUUIDStringKey,
            .volumeLocalizedNameKey
        ]
        guard let urls = FileManager.default.mountedVolumeURLs(
            includingResourceValuesForKeys: Array(keys),
            options: [.skipHiddenVolumes]
        ) else {
            throw SourceDiscoveryError.enumerationUnavailable
        }
        return urls.compactMap { url in
            guard let values = try? url.resourceValues(forKeys: keys) else {
                return nil
            }
            return MountedVolume(
                rootURL: url,
                localizedName: values.volumeLocalizedName,
                uuid: values.volumeUUIDString,
                isReadable: values.isReadable,
                isRemovable: values.volumeIsRemovable,
                isEjectable: values.volumeIsEjectable
            )
        }
    }

    nonisolated private static func systemCatalogVolume(_ path: String) -> URL? {
        try? URL(fileURLWithPath: path)
            .resourceValues(forKeys: [.volumeURLKey])
            .volume
    }
}

#if DEBUG
@MainActor
final class DebugSourceDiscovery: SourceDiscoveryClient {
    enum Scenario: String {
        case none
        case connected
        case multiple
        case disconnectBeforeReview = "disconnect-before-review"
        case errorThenConnected = "error-then-connected"
    }

    private let scenario: Scenario
    private var discoveryCount = 0

    init(scenario: Scenario) {
        self.scenario = scenario
    }

    func discoverSources(catalogPath: String) async throws -> [ConnectedSource] {
        discoveryCount += 1
        switch scenario {
        case .none:
            return []
        case .connected:
            return [Self.camera]
        case .multiple:
            return [Self.backupCard, Self.camera]
        case .disconnectBeforeReview:
            return discoveryCount == 1 ? [Self.camera] : []
        case .errorThenConnected:
            if discoveryCount == 1 {
                throw SourceDiscoveryError.enumerationUnavailable
            }
            return [Self.camera]
        }
    }

    func startObserving(_ onChange: @escaping @MainActor () -> Void) {}

    func stopObserving() {}

    private static let camera = ConnectedSource(
        id: "uuid:eos-digital",
        rootPath: "/Volumes/EOS_DIGITAL",
        name: "EOS_DIGITAL"
    )

    private static let backupCard = ConnectedSource(
        id: "uuid:backup-card",
        rootPath: "/Volumes/BACKUP_CARD",
        name: "BACKUP_CARD"
    )
}

@MainActor
enum SourceDiscoveryFactory {
    static func make(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> SourceDiscoveryClient {
        if let value = environment["SOUVENIRS_SOURCE_DISCOVERY_SCENARIO"],
           let scenario = DebugSourceDiscovery.Scenario(rawValue: value) {
            return DebugSourceDiscovery(scenario: scenario)
        }
        return MountedSourceDiscovery()
    }
}
#else
@MainActor
enum SourceDiscoveryFactory {
    static func make() -> SourceDiscoveryClient {
        MountedSourceDiscovery()
    }
}
#endif
