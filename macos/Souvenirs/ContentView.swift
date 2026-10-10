import SwiftUI

struct ContentView: View {
    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "photo.on.rectangle.angled")
                .font(.system(size: 40))
                .foregroundStyle(.secondary)

            Text("Souvenirs")
                .font(.largeTitle)

            Text("Your photo and video catalog")
                .foregroundStyle(.secondary)
        }
        .frame(minWidth: 520, minHeight: 320)
        .accessibilityIdentifier("souvenirs-welcome")
    }
}

#Preview {
    ContentView()
}
