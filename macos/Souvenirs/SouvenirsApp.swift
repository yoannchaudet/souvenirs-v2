import AppKit
import Combine
import SwiftUI

struct EngineLaunchConfiguration: Equatable {
    let executableURL: URL
    let arguments: [String]
}

@MainActor
final class EngineAppModel: ObservableObject {
    enum Status: Equatable {
        case connecting
        case ready(engineVersion: String)
        case failed(String)
    }

    @Published private(set) var status: Status = .connecting

    let supervisor: EngineSupervisor
    private var lifecycleTask: Task<Void, Never>?
    private var isStopping = false

    init(environment: [String: String] = ProcessInfo.processInfo.environment) {
        if let configuration = Self.fakeEngineConfiguration(environment: environment) {
            supervisor = EngineSupervisor(
                executableURL: configuration.executableURL,
                arguments: configuration.arguments
            )
        } else {
            supervisor = EngineSupervisor()
        }
    }

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

    func start() {
        guard lifecycleTask == nil else {
            return
        }

        lifecycleTask = Task {
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
            EngineRootView(engine: appDelegate.engine)
        }
    }
}

private struct EngineRootView: View {
    @ObservedObject var engine: EngineAppModel
    @StateObject private var welcome: CatalogWelcomeModel

    init(engine: EngineAppModel) {
        self.engine = engine
        _welcome = StateObject(wrappedValue: CatalogWelcomeModel(engine: engine))
    }

    var body: some View {
        ContentView(engineStatus: engine.status, model: welcome)
    }
}
