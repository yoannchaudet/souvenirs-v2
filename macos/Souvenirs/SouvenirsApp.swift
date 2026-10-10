import AppKit
import Combine
import SwiftUI

@MainActor
final class EngineAppModel: ObservableObject {
    enum Status: Equatable {
        case connecting
        case ready(engineVersion: String)
        case failed(String)
    }

    @Published private(set) var status: Status = .connecting

    private let supervisor = EngineSupervisor()
    private var lifecycleTask: Task<Void, Never>?
    private var isStopping = false

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

    var body: some View {
        ContentView(engineStatus: engine.status)
    }
}
