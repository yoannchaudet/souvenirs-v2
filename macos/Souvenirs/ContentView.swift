import AppKit
import SwiftUI

struct ContentView: View {
    let engineStatus: EngineAppModel.Status
    @ObservedObject var model: CatalogWelcomeModel
    @State private var showsCreatedItems = false

    var body: some View {
        Group {
            switch model.route {
            case .welcome:
                welcome
            case .create:
                createCatalog
            case .catalog(let catalog):
                catalogHome(catalog)
            }
        }
        .frame(minWidth: 586, minHeight: 386)
        .background(.ultraThinMaterial)
        .alert(
            model.presentedErrorTitle,
            isPresented: Binding(
                get: { model.presentedError != nil },
                set: { if !$0 { model.presentedError = nil } }
            )
        ) {
            Button("OK") { model.presentedError = nil }
                .accessibilityIdentifier("catalog-error-dismiss")
        } message: {
            Text(model.presentedError ?? "")
        }
    }

    private var welcome: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                Spacer()
                Image(systemName: "photo.on.rectangle.angled")
                    .font(.system(size: 30, weight: .medium))
                    .foregroundStyle(.yellow)
                    .frame(width: 74, height: 74)
                    .background(.black, in: RoundedRectangle(cornerRadius: 18))
                    .shadow(color: .black.opacity(0.18), radius: 16, y: 8)

                Text("Welcome to Souvenirs")
                    .font(.system(size: 24, weight: .bold))
                    .padding(.top, 15)
                    .accessibilityIdentifier("souvenirs-welcome")

                Text("Keep your photos and videos as ordinary folders\nyou can open anywhere, for decades.")
                    .font(.system(size: 13))
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .lineSpacing(2)
                    .padding(.top, 5)

                VStack(spacing: 8) {
                    welcomeAction(
                        title: "Create a New Catalog…",
                        subtitle: "Pick a folder on a drive or network share",
                        systemImage: "plus",
                        prominent: true,
                        identifier: "create-catalog"
                    ) {
                        model.beginCreate()
                    }
                    welcomeAction(
                        title: "Open an Existing Catalog…",
                        subtitle: "Including a backup copy",
                        systemImage: "folder",
                        prominent: false,
                        identifier: "open-catalog"
                    ) {
                        chooseFolder { path in
                            Task { await model.openCatalog(at: path) }
                        }
                    }
                }
                .padding(.top, 20)
                Spacer()
            }
            .frame(width: 330)
            .padding(.horizontal, 28)

            Divider()

            VStack(alignment: .leading, spacing: 8) {
                Text("Recent")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.bottom, 2)

                if model.recents.isEmpty {
                    Text("Catalogs you open will appear here.")
                        .font(.callout)
                        .foregroundStyle(.tertiary)
                        .padding(.top, 8)
                } else {
                    ForEach(model.recents) { catalog in
                        Button {
                            Task {
                                await model.openCatalog(
                                    at: catalog.path,
                                    name: catalog.name
                                )
                            }
                        } label: {
                            HStack(spacing: 10) {
                                Image(systemName: "externaldrive")
                                    .font(.system(size: 15))
                                    .frame(width: 28, height: 28)
                                    .background(.background.opacity(0.8), in: RoundedRectangle(cornerRadius: 7))
                                VStack(alignment: .leading, spacing: 1) {
                                    Text(catalog.name)
                                        .fontWeight(.medium)
                                        .foregroundStyle(.primary)
                                    Text(catalog.volumeName)
                                        .font(.caption)
                                        .foregroundStyle(.secondary)
                                }
                                Spacer()
                            }
                            .padding(7)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("recent-catalog-\(catalog.id)")
                        .contextMenu {
                            Button("Remove from Recents") {
                                model.removeRecent(path: catalog.path)
                            }
                            .accessibilityIdentifier("remove-recent-\(catalog.id)")
                        }
                    }
                }
                Spacer()
                engineBadge
            }
            .padding(20)
            .frame(width: 255)
            .background(Color(nsColor: .controlBackgroundColor).opacity(0.48))
        }
        .frame(width: 586, height: 386)
    }

    private var createCatalog: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 15) {
                Text("New Catalog")
                    .font(.title2.bold())
                Text("Choose where your photos will live. You can open this folder in Finder at any time.")
                    .font(.callout)
                    .foregroundStyle(.secondary)

                VStack(alignment: .leading, spacing: 6) {
                    Text("Name").font(.caption)
                    TextField("Catalog name", text: $model.catalogName)
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("catalog-name")
                }

                VStack(alignment: .leading, spacing: 6) {
                    Text("Location").font(.caption)
                    HStack(spacing: 8) {
                        HStack(spacing: 9) {
                            Image(systemName: "externaldrive")
                            VStack(alignment: .leading, spacing: 1) {
                                Text(locationTitle)
                                    .fontWeight(.medium)
                                    .lineLimit(1)
                                Text(locationSubtitle)
                                    .font(.caption)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                            }
                            Spacer()
                        }
                        .padding(.horizontal, 11)
                        .frame(height: 48)
                        .background(.background.opacity(0.72), in: RoundedRectangle(cornerRadius: 9))
                        .overlay {
                            RoundedRectangle(cornerRadius: 9)
                                .stroke(.separator.opacity(0.65))
                        }

                        Button("Choose…") {
                            chooseFolder { path in
                                Task { await model.chooseCreateLocation(path) }
                            }
                        }
                        .accessibilityIdentifier("choose-catalog-location")
                    }
                    validationLine
                }

                DisclosureGroup("What will be created", isExpanded: $showsCreatedItems) {
                    Text(".souvenirs.yml and a small .souvenirs folder. Your photos are added later, by import.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .padding(.top, 4)
                }
                .accessibilityIdentifier("created-items-disclosure")
                .padding(12)
                .background(.quaternary.opacity(0.55), in: RoundedRectangle(cornerRadius: 10))

                if case .invalid(let message) = model.validation {
                    VStack(alignment: .leading, spacing: 5) {
                        Text("This location can’t be used")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Label(message, systemImage: "exclamationmark.circle")
                            .font(.caption)
                            .foregroundStyle(.red)
                    }
                    .padding(12)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.red.opacity(0.035), in: RoundedRectangle(cornerRadius: 10))
                    .overlay {
                        RoundedRectangle(cornerRadius: 10)
                            .stroke(.red.opacity(0.35), style: StrokeStyle(lineWidth: 1, dash: [3]))
                    }
                    .accessibilityIdentifier("catalog-location-error")
                }
                Spacer(minLength: 0)
            }
            .padding(24)

            Divider()
            HStack {
                Spacer()
                Button("Cancel") { model.cancelCreate() }
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("cancel-create-catalog")
                Button("Create Catalog") {
                    Task { await model.createCatalog() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canCreate)
                .accessibilityIdentifier("confirm-create-catalog")
            }
            .padding(16)
        }
        .frame(width: 465, height: 455)
        .background(.regularMaterial)
    }

    private func catalogHome(_ catalog: RecentCatalog) -> some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text(catalog.name)
                    .font(.headline)
                Text(catalog.volumeName)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text("Library")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.top, 16)
                Label("All Photos", systemImage: "square.grid.2x2")
                    .padding(7)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.selection.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
                Spacer()
                Divider()
                Label("Ready for your first import", systemImage: "circle.fill")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(.green, .secondary)
                    .accessibilityIdentifier("catalog-health")
            }
            .padding(16)
            .frame(width: 185)
            .background(Color(nsColor: .underPageBackgroundColor))

            Divider()

            VStack {
                HStack {
                    Text("All Photos")
                        .font(.headline)
                    Spacer()
                }
                .padding(.horizontal, 18)
                .frame(height: 44)
                Divider()
                Spacer()
                Image(systemName: "square.and.arrow.down")
                    .font(.system(size: 28))
                    .foregroundStyle(.blue)
                    .frame(width: 84, height: 66)
                    .background(.background, in: RoundedRectangle(cornerRadius: 12))
                    .shadow(color: .black.opacity(0.09), radius: 14, y: 7)
                Text("Your catalog is ready")
                    .font(.title2.bold())
                    .padding(.top, 18)
                    .accessibilityIdentifier("catalog-ready")
                Text("Insert a memory card, or choose a folder of photos. Souvenirs will\nshow you exactly what it plans to copy before anything happens.")
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .padding(.top, 3)
                Button("Choose a Folder to Import…") {}
                    .buttonStyle(.borderedProminent)
                    .disabled(true)
                    .accessibilityIdentifier("choose-import-folder")
                    .padding(.top, 10)
                Text("Memory cards appear in the sidebar when you connect them.")
                    .font(.caption)
                    .foregroundStyle(.tertiary)
                    .padding(.top, 4)
                Spacer()
                HStack(spacing: 3) {
                    Image(systemName: "folder")
                    Text("Stored as ordinary folders on \(catalog.volumeName) ·")
                    Button("Show in Finder") {
                        NSWorkspace.shared.activateFileViewerSelecting([
                            URL(fileURLWithPath: catalog.path)
                        ])
                    }
                    .buttonStyle(.link)
                    .accessibilityIdentifier("show-catalog-in-finder")
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.bottom, 14)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(minWidth: 820, minHeight: 560)
    }

    private func welcomeAction(
        title: String,
        subtitle: String,
        systemImage: String,
        prominent: Bool,
        identifier: String,
        action: @escaping () -> Void
    ) -> some View {
        Button(action: action) {
            HStack(spacing: 10) {
                Image(systemName: systemImage).frame(width: 18)
                VStack(alignment: .leading, spacing: 1) {
                    Text(title).fontWeight(.medium)
                    Text(subtitle).font(.caption).opacity(0.72)
                }
                Spacer()
            }
            .padding(.horizontal, 12)
            .frame(height: 50)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .foregroundStyle(prominent ? Color.white : Color.primary)
        .background(
            prominent ? Color.accentColor : Color(nsColor: .controlColor),
            in: RoundedRectangle(cornerRadius: 10)
        )
        .accessibilityIdentifier(identifier)
    }

    @ViewBuilder
    private var validationLine: some View {
        switch model.validation {
        case .none:
            EmptyView()
        case .checking:
            Label("Checking this location…", systemImage: "ellipsis.circle")
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("catalog-location-checking")
        case .valid:
            Label("This location works. Other Macs can open it too, one at a time.", systemImage: "checkmark")
                .foregroundStyle(.green)
                .accessibilityIdentifier("catalog-location-valid")
        case .invalid:
            EmptyView()
        }
    }

    private var locationTitle: String {
        model.selectedPath.map { URL(fileURLWithPath: $0).lastPathComponent }
            ?? "Choose a folder"
    }

    private var locationSubtitle: String {
        switch model.validation {
        case .valid(let volumeName): return volumeName
        default: return model.selectedPath ?? "Local or network drive"
        }
    }

    private var engineBadge: some View {
        Group {
            switch engineStatus {
            case .connecting:
                Label("Connecting…", systemImage: "ellipsis.circle")
            case .ready:
                Label("Engine ready", systemImage: "checkmark.circle.fill")
            case .failed(let message):
                Label(message, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.red)
            }
        }
        .font(.caption)
        .foregroundStyle(.secondary)
        .accessibilityIdentifier("engine-status")
    }

    private func chooseFolder(onSelection: @escaping (String) -> Void) {
        #if DEBUG
        if let testPath = ProcessInfo.processInfo.environment["SOUVENIRS_UI_TEST_DIRECTORY"] {
            onSelection(testPath)
            return
        }
        #endif
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = "Choose"
        if panel.runModal() == .OK, let url = panel.url {
            onSelection(url.path)
        }
    }
}

#Preview {
    ContentView(
        engineStatus: .ready(engineVersion: "0.1.0"),
        model: CatalogWelcomeModel(
            engine: PreviewCatalogEngine(),
            recents: [
                RecentCatalog(
                    path: "/Volumes/photos/Family Catalog",
                    name: "Family Catalog",
                    volumeName: "photos-nas.local"
                )
            ]
        )
    )
}

@MainActor
private final class PreviewCatalogEngine: CatalogEngineClient {
    func validateCatalog(path: String, intent: String) async throws
        -> EngineSupervisor.CatalogLocation {
        .init(
            path: path,
            volumeName: "photos-nas.local",
            displayName: "Family Catalog",
            detail: "Catalog format 2"
        )
    }

    func createCatalog(
        path: String,
        displayName: String
    ) async throws -> EngineSupervisor.CatalogLocation {
        .init(
            path: path,
            volumeName: "photos-nas.local",
            displayName: displayName,
            detail: "Catalog format 2"
        )
    }

    func openCatalog(path: String) async throws -> EngineSupervisor.CatalogLocation {
        .init(
            path: path,
            volumeName: "photos-nas.local",
            displayName: "Family Catalog",
            detail: "Catalog format 2"
        )
    }
}
