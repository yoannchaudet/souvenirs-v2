import SwiftUI

struct ContentView: View {
    let engineStatus: EngineAppModel.Status

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "photo.on.rectangle.angled")
                .font(.system(size: 40))
                .foregroundStyle(.secondary)

            Text("Souvenirs")
                .font(.largeTitle)
                .accessibilityIdentifier("souvenirs-welcome")

            Text("Your photo and video catalog")
                .foregroundStyle(.secondary)

            engineStatusView
                .padding(.top, 12)
        }
        .frame(minWidth: 520, minHeight: 320)
    }

    @ViewBuilder
    private var engineStatusView: some View {
        switch engineStatus {
        case .connecting:
            Label("Connecting to engine…", systemImage: "ellipsis.circle")
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("engine-status")
        case .ready:
            Label("Engine ready", systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green)
                .accessibilityIdentifier("engine-status")
        case .failed(let message):
            VStack(spacing: 4) {
                Label("Engine unavailable", systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Engine error: \(message)")
            .accessibilityIdentifier("engine-status")
        }
    }
}

#Preview {
    ContentView(engineStatus: .ready(engineVersion: "0.1.0"))
}
