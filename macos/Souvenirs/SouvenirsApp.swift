import AppKit
import Combine
import SwiftUI

#if DEBUG
struct EngineLaunchConfiguration: Equatable {
    let executableURL: URL
    let arguments: [String]
}
#endif

@MainActor
final class EngineAppModel: ObservableObject {
    enum Status: Equatable {
        case connecting
        case ready(engineVersion: String)
        case failed(String)

        var catalogActionsAvailable: Bool {
            if case .ready = self {
                return true
            }
            return false
        }

        var catalogActionsUnavailableDescription: String? {
            switch self {
            case .connecting:
                return "Souvenirs is still connecting to its engine."
            case .ready:
                return nil
            case .failed(let message):
                return "The Souvenirs engine is unavailable. \(message)"
            }
        }
    }

    @Published private(set) var status: Status = .connecting

    let supervisor: EngineSupervisor
    private var lifecycleTask: Task<Void, Never>?
    private var isStopping = false

    init(environment: [String: String] = ProcessInfo.processInfo.environment) {
        #if DEBUG
        if let configuration = Self.fakeEngineConfiguration(environment: environment) {
            supervisor = EngineSupervisor(
                executableURL: configuration.executableURL,
                arguments: configuration.arguments
            )
            return
        }
        #endif
        supervisor = EngineSupervisor()
    }

    #if DEBUG
    static func fakeEngineConfiguration(
        environment: [String: String]
    ) -> EngineLaunchConfiguration? {
        guard let executable = environment["SOUVENIRS_ENGINE_EXECUTABLE"],
              let scenario = environment["SOUVENIRS_FAKE_ENGINE_SCENARIO"] else {
            return nil
        }
        return EngineLaunchConfiguration(
            executableURL: URL(fileURLWithPath: executable),
            arguments: ["--scenario", scenario]
        )
    }
    #endif

    func start() {
        guard lifecycleTask == nil, !isStopping else {
            return
        }

        status = .connecting
        lifecycleTask = Task {
            defer { lifecycleTask = nil }
            do {
                guard !Task.isCancelled else {
                    return
                }

                _ = try await supervisor.start()
                if Task.isCancelled {
                    try? await supervisor.stop()
                    return
                }

                let handshake = try await supervisor.handshake()
                status = .ready(engineVersion: handshake.engineVersion)

                let termination = try await supervisor.waitForTermination()
                if !isStopping {
                    status = .failed(
                        "The engine stopped unexpectedly with status \(termination.status)."
                    )
                }
            } catch {
                if !isStopping {
                    try? await supervisor.stop()
                    status = .failed(error.localizedDescription)
                }
            }
        }
    }

    func retry() {
        guard case .failed = status else {
            return
        }
        start()
    }

    func stop() async {
        guard !isStopping else {
            return
        }

        isStopping = true
        lifecycleTask?.cancel()
        try? await supervisor.stop()
        _ = await lifecycleTask?.result
        lifecycleTask = nil
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let engine = EngineAppModel()
    lazy var imports = ImportExecutionCoordinator(engine: engine)

    private var terminationRequested = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        engine.start()
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !terminationRequested else {
            return .terminateLater
        }

        terminationRequested = true
        Task {
            await engine.stop()
            sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}

@main
struct SouvenirsApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate

    var body: some Scene {
        WindowGroup {
            EngineRootView(
                engine: appDelegate.engine,
                imports: appDelegate.imports
            )
        }
        .defaultSize(width: 920, height: 620)
        .windowResizability(.contentMinSize)
    }
}

private struct EngineRootView: View {
    @ObservedObject var engine: EngineAppModel
    @ObservedObject var imports: ImportExecutionCoordinator
    @StateObject private var welcome: CatalogWelcomeModel

    init(engine: EngineAppModel, imports: ImportExecutionCoordinator) {
        self.engine = engine
        self.imports = imports
        _welcome = StateObject(
            wrappedValue: CatalogWelcomeModel(
                engine: engine,
                sourceDiscovery: SourceDiscoveryFactory.make()
            )
        )
    }

    var body: some View {
        ContentView(
            engineStatus: engine.status,
            model: welcome,
            imports: imports,
            onRetryEngine: engine.retry
        )
    }
}
