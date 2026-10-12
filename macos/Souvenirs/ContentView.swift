import AppKit
import SwiftUI

struct ContentView: View {
    private enum ModalReturnFocus: Hashable {
        case createCatalog
        case chooseImportFolder
        case connectedSource
        case sourceStatus
        case retryDeferredRecovery
        case stopImport
    }

    let engineStatus: EngineAppModel.Status
    @ObservedObject var model: CatalogWelcomeModel
    @ObservedObject var imports: ImportExecutionCoordinator
    let onRetryEngine: (() -> Void)?
    @State private var modalReturnFocus: ModalReturnFocus?
    @State private var connectedReviewRequestID: UUID?
    @FocusState private var keyboardFocus: ModalReturnFocus?
    @AccessibilityFocusState private var accessibilityFocus: ModalReturnFocus?

    init(
        engineStatus: EngineAppModel.Status,
        model: CatalogWelcomeModel,
        imports: ImportExecutionCoordinator,
        onRetryEngine: (() -> Void)? = nil
    ) {
        self.engineStatus = engineStatus
        self.model = model
        self.imports = imports
        self.onRetryEngine = onRetryEngine
    }

    var body: some View {
        Group {
            switch model.route {
            case .welcome, .create:
                welcome
            case .catalog(let catalog):
                catalogHome(catalog)
            }
        }
        .frame(minWidth: 586, minHeight: 386)
        .background(.ultraThinMaterial)
        .sheet(
            isPresented: createCatalogPresented,
            onDismiss: restoreModalFocus
        ) {
            CatalogCreationSheet(
                model: model,
                onChooseLocation: {
                    chooseFolder { path in
                        Task { await model.chooseCreateLocation(path) }
                    }
                },
                onCancel: model.cancelCreate,
                onCreate: {
                    Task { await model.createCatalog() }
                }
            )
            .frame(width: 540, height: 500)
        }
        .sheet(
            isPresented: importPlanningPresented,
            onDismiss: restoreModalFocus
        ) {
            if let planning = model.importPlanning {
                ImportPlanningSheet(
                    model: planning,
                    onCancel: model.dismissImport,
                    onStart: {
                        guard let review = planning.review else { return }
                        modalReturnFocus = .stopImport
                        imports.start(review: review, catalog: planning.catalog)
                        model.dismissImport()
                    }
                )
                .frame(width: 540, height: 500)
            }
        }
        .sheet(
            isPresented: recoveryPresented,
            onDismiss: restoreModalFocus
        ) {
            if case .recoveryPrompt(let sourceAvailable) = imports.phase {
                RecoverySheet(
                    imports: imports,
                    sourceAvailable: sourceAvailable,
                    onStop: {
                        modalReturnFocus = nil
                        imports.stopRecoveryHere()
                    },
                    onDefer: {
                        modalReturnFocus = .retryDeferredRecovery
                        imports.deferRecovery()
                    },
                    onRetry: imports.retryRecovery,
                    onFinish: {
                        modalReturnFocus = .stopImport
                        imports.finishRecovery()
                    }
                )
                .frame(width: 465, height: 320)
            }
        }
        .alert(
            model.presentedErrorTitle,
            isPresented: Binding(
                get: {
                    guard model.presentedError != nil else { return false }
                    if case .create = model.route { return false }
                    return true
                },
                set: { if !$0 { model.presentedError = nil } }
            )
        ) {
            Button("OK") { model.presentedError = nil }
                .accessibilityIdentifier("catalog-error-dismiss")
        } message: {
            Text(model.presentedError ?? "")
        }
    }

    private var createCatalogPresented: Binding<Bool> {
        Binding(
            get: {
                if case .create = model.route { return true }
                return false
            },
            set: { isPresented in
                if !isPresented, case .create = model.route {
                    model.cancelCreate()
                }
            }
        )
    }

    private var importPlanningPresented: Binding<Bool> {
        Binding(
            get: { model.importPlanning != nil },
            set: { isPresented in
                if !isPresented {
                    model.dismissImport()
                }
            }
        )
    }

    private var recoveryPresented: Binding<Bool> {
        Binding(
            get: {
                if case .recoveryPrompt = imports.phase { return true }
                return false
            },
            set: { isPresented in
                if !isPresented, case .recoveryPrompt = imports.phase {
                    modalReturnFocus = .retryDeferredRecovery
                    imports.deferRecovery()
                }
            }
        )
    }

    private func restoreModalFocus() {
        guard var target = modalReturnFocus else { return }
        modalReturnFocus = nil
        if target == .createCatalog, model.route != .welcome {
            return
        }
        if target == .connectedSource, model.selectedConnectedSource == nil {
            target = .sourceStatus
        }
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(150))
            keyboardFocus = target
            accessibilityFocus = target
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
                        title: "Create a New Archive…",
                        subtitle: "Pick a folder on a drive or network share",
                        systemImage: "plus",
                        prominent: true,
                        identifier: "create-catalog"
                    ) {
                        guard engineStatus.catalogActionsAvailable else { return }
                        modalReturnFocus = .createCatalog
                        model.beginCreate()
                    }
                    .focusable()
                    .focused($keyboardFocus, equals: .createCatalog)
                    .accessibilityFocused(
                        $accessibilityFocus,
                        equals: .createCatalog
                    )
                    welcomeAction(
                        title: "Open an Existing Archive…",
                        subtitle: "Including a backup copy",
                        systemImage: "folder",
                        prominent: false,
                        identifier: "open-catalog"
                    ) {
                        guard engineStatus.catalogActionsAvailable else { return }
                        chooseFolder { path in
                            Task { await model.openCatalog(at: path) }
                        }
                    }
                }
                .frame(maxWidth: 330)
                .padding(.top, 20)
                Spacer()
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .padding(.horizontal, 20)

            Divider()

            VStack(alignment: .leading, spacing: 8) {
                Text("Recent")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.bottom, 2)

                if model.recents.isEmpty {
                    Text("Archives you open will appear here.")
                        .font(.callout)
                        .foregroundStyle(.tertiary)
                        .padding(.top, 8)
                } else {
                    ForEach(model.recents) { catalog in
                        Button {
                            guard engineStatus.catalogActionsAvailable else { return }
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
                        .disabled(!engineStatus.catalogActionsAvailable)
                        .accessibilityHint(catalogActionAccessibilityHint)
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
                catalogActionsUnavailableNotice
                engineBadge
            }
            .padding(20)
            .frame(width: 255)
            .background(Color(nsColor: .controlBackgroundColor).opacity(0.48))
        }
        .frame(minWidth: 586, minHeight: 386)
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
                CatalogBrowseSidebar(model: model.browse)
                    .frame(maxHeight: .infinity)
                Text("Import from")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.top, 8)
                connectedSourceSidebar
                Divider()
                Label(sidebarStatus, systemImage: "circle.fill")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .symbolRenderingMode(.palette)
                    .foregroundStyle(sidebarStatusColor, .secondary)
                    .accessibilityIdentifier("catalog-health")
            }
            .padding(16)
            .frame(width: 185)
            .background(Color(nsColor: .underPageBackgroundColor))

            Divider()

            VStack(spacing: 0) {
                HStack {
                    CatalogBrowseHeader(model: model.browse)
                    Spacer()
                    if imports.phase == .idle || imports.phase == .recoveryDeferred {
                        Button {
                            Task {
                                async let browseRefresh: Void = model.browse.refresh()
                                model.refreshConnectedSources()
                                await browseRefresh
                            }
                        } label: {
                            Label("Refresh", systemImage: "arrow.clockwise")
                                .labelStyle(.iconOnly)
                        }
                        .help("Refresh archive and connected sources")
                        .accessibilityIdentifier("archive-browse-refresh")

                        Button("Import…") {
                            chooseFolder { path in
                                modalReturnFocus = .chooseImportFolder
                                model.beginImport(from: path, into: catalog)
                            }
                        }
                        .disabled(
                            imports.phase == .recoveryDeferred
                                || model.importPlanning != nil
                        )
                        .accessibilityIdentifier("choose-import-folder")
                        .focusable()
                        .focused($keyboardFocus, equals: .chooseImportFolder)
                        .accessibilityFocused(
                            $accessibilityFocus,
                            equals: .chooseImportFolder
                        )
                    }
                }
                .padding(.horizontal, 18)
                .frame(height: 44)
                Divider()
                VStack(spacing: 0) {
                    if imports.catalog?.path == catalog.path {
                        importActivity
                    }
                    if imports.catalog?.path == catalog.path,
                       imports.phase == .recoveryDeferred {
                        deferredRecovery
                            .padding(.vertical, 12)
                    }
                    if imports.phase == .idle || imports.phase == .recoveryDeferred,
                       let source = model.selectedConnectedSource {
                        connectedSourceBanner(source, catalog: catalog)
                            .padding(.horizontal, 18)
                            .padding(.vertical, 12)
                    }
                    CatalogBrowsePane(model: model.browse) {
                        catalogReadyContent(catalog)
                    }
                }
                HStack(spacing: 3) {
                    Image(systemName: "folder")
                    Text("Stored as ordinary folders on \(catalog.volumeName) ·")
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Button("Show in Finder") {
                        NSWorkspace.shared.activateFileViewerSelecting([
                            URL(fileURLWithPath: catalog.path)
                        ])
                    }

                    .buttonStyle(.link)
                    .layoutPriority(1)
                    .accessibilityIdentifier("show-catalog-in-finder")
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                .padding(.horizontal, 18)
                .frame(minHeight: 34)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .task(id: catalog.path) {
            await imports.inspect(catalog: catalog)
        }
        .task(id: "browse-\(catalog.path)") {
            await model.browse.load(catalogPath: catalog.path)
        }
        .onChange(of: imports.presentationRevision, initial: true) { _, _ in
            Task { await syncImportPresentation(for: catalog) }
        }
        .task(id: terminalRefreshToken) {
            guard terminalRefreshToken != nil,
                  imports.catalog?.path == catalog.path else {
                return
            }
            await syncImportPresentation(for: catalog)
            await model.browse.reconcileTerminalImport()
        }
    }

    @ViewBuilder
    private var connectedSourceSidebar: some View {
        if let source = model.selectedConnectedSource {
            HStack(spacing: 8) {
                HStack(spacing: 8) {
                    Image(systemName: "externaldrive.fill")
                        .foregroundStyle(.blue)
                    VStack(alignment: .leading, spacing: 1) {
                        Text(source.name)
                            .font(.caption.weight(.medium))
                            .lineLimit(1)
                        Text("Ready to review")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("connected-source-card")
                .accessibilityLabel(source.name)
                .accessibilityValue("Connected, ready to review")
                Spacer(minLength: 0)
                if model.connectedSources.count > 1 {
                    Menu {
                        ForEach(model.connectedSources) { candidate in
                            Button {
                                model.selectConnectedSource(id: candidate.id)
                            } label: {
                                if candidate.id == source.id {
                                    Label(candidate.name, systemImage: "checkmark")
                                } else {
                                    Text(candidate.name)
                                }
                            }
                            .accessibilityIdentifier(
                                "connected-source-option-\(candidate.id)"
                            )
                        }
                    } label: {
                        Image(systemName: "chevron.up.chevron.down")
                            .font(.caption2)
                    }
                    .menuStyle(.borderlessButton)
                    .menuIndicator(.hidden)
                    .accessibilityLabel("Choose connected source")
                    .accessibilityIdentifier("connected-source-selector")
                }
            }
            .padding(8)
            .contentShape(Rectangle())
            .background(.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 8))
            .overlay {
                RoundedRectangle(cornerRadius: 8)
                    .stroke(.separator.opacity(0.55))
            }
        } else {
            VStack(alignment: .leading, spacing: 5) {
                Text(sourceStatusLine)
                    .font(.caption2)
                    .foregroundStyle(
                        model.sourceDiscoveryError == nil ? Color.secondary : Color.red
                    )
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("connected-source-status")
                    .accessibilityLabel("Connected source status")
                    .accessibilityValue(sourceStatusLine)
                Button(
                    model.sourceDiscoveryError == nil ? "Refresh" : "Try Again"
                ) {
                    model.retrySourceDiscovery()
                }
                .font(.caption)
                .disabled(model.isDiscoveringSources)
                .accessibilityIdentifier("connected-source-retry")
                .focusable()
                .focused($keyboardFocus, equals: .sourceStatus)
                .accessibilityFocused($accessibilityFocus, equals: .sourceStatus)
            }
            .padding(.vertical, 5)
        }
    }

    private var sourceStatusLine: String {
        if model.isDiscoveringSources {
            return "Checking connected sources…"
        }
        if let sourceDiscoveryError = model.sourceDiscoveryError {
            return sourceDiscoveryError
        }
        return "No readable removable source connected"
    }

    private func connectedSourceBanner(
        _ source: ConnectedSource,
        catalog: RecentCatalog
    ) -> some View {
        HStack(spacing: 12) {
            Image(systemName: "externaldrive.badge.checkmark")
                .font(.title2)
                .foregroundStyle(.blue)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(sourceSummary(source))
                    .font(.callout.weight(.semibold))
                Text("Souvenirs scans this source only after you review the import.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("connected-source-summary")
            .accessibilityLabel("Connected source")
            .accessibilityValue(sourceSummary(source))
            .accessibilityHint(
                "Souvenirs scans this source only after you review the import."
            )
            Spacer(minLength: 8)
            Button("Review Import…") {
                let requestID = UUID()
                connectedReviewRequestID = requestID
                modalReturnFocus = .connectedSource
                Task {
                    let started = await model.reviewConnectedSource(
                        id: source.id,
                        into: catalog
                    )
                    guard connectedReviewRequestID == requestID else { return }
                    connectedReviewRequestID = nil
                    guard !started else { return }
                    if modalReturnFocus == .connectedSource {
                        modalReturnFocus = nil
                    }
                    await Task.yield()
                    try? await Task.sleep(for: .milliseconds(150))
                    let target: ModalReturnFocus =
                        model.selectedConnectedSource == nil
                        ? .sourceStatus : .connectedSource
                    keyboardFocus = target
                    accessibilityFocus = target
                }
            }
            .buttonStyle(.borderedProminent)
            .disabled(
                imports.phase != .idle
                    || model.importPlanning != nil
                    || model.isDiscoveringSources
            )
            .accessibilityIdentifier("review-connected-source")
            .focusable()
            .focused($keyboardFocus, equals: .connectedSource)
            .accessibilityFocused($accessibilityFocus, equals: .connectedSource)
        }
        .padding(12)
        .background(.blue.opacity(0.08), in: RoundedRectangle(cornerRadius: 10))
        .overlay {
            RoundedRectangle(cornerRadius: 10)
                .stroke(.blue.opacity(0.28))
        }
    }

    private func sourceSummary(_ source: ConnectedSource) -> String {
        let count = model.connectedSources.count
        if count == 1 {
            return "1 removable source connected · \(source.name)"
        }
        return "\(count) removable sources connected · \(source.name) selected"
    }

    private func catalogReadyContent(_ catalog: RecentCatalog) -> some View {
        GeometryReader { proxy in
            ScrollView {
                VStack(spacing: 0) {
                    Spacer(minLength: 24)
                    Image(systemName: "square.and.arrow.down")
                        .font(.system(size: 28))
                        .foregroundStyle(.blue)
                        .frame(width: 84, height: 66)
                        .background(.background, in: RoundedRectangle(cornerRadius: 12))
                        .shadow(color: .black.opacity(0.09), radius: 14, y: 7)
                    Text("Your archive is ready")
                        .font(.title2.bold())
                        .padding(.top, 18)
                        .accessibilityIdentifier("catalog-ready")
                    Text("Connect a mounted memory card, or choose a folder of photos. Souvenirs will show you exactly what it plans to copy before anything happens.")
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 560)
                        .padding(.top, 3)
                    Text("Readable removable volumes appear in the sidebar when they mount.")
                        .font(.caption)
                        .foregroundStyle(.tertiary)
                        .padding(.top, 4)
                    Spacer(minLength: 24)
                }
                .padding(.horizontal, 18)
                .frame(maxWidth: .infinity, minHeight: proxy.size.height)
            }
        }
    }

    private var sidebarStatus: String {
        switch imports.phase {
        case .running, .recovering, .stopping:
            return "Importing · \(imports.completedUnits) of \(imports.totalUnits)"
        case .recoveryPrompt, .recoveryDeferred:
            return "Import paused · \(imports.completedUnits) of \(imports.totalUnits) safe"
        case .completed, .cancelled:
            switch model.browse.importReconciliation {
            case .landed:
                return "Everything is safe"
            case .failed:
                return "Archive access · needs you"
            case .active, .refreshing, .none:
                return "Updating archive…"
            }
        case .failed:
            return "Import paused · needs you"
        case .idle:
            return "Everything is safe"
        }
    }

    private var sidebarStatusColor: Color {
        switch imports.phase {
        case .running, .recovering, .stopping:
            return .blue
        case .recoveryPrompt, .recoveryDeferred:
            return .orange
        case .completed, .cancelled:
            if case .failed = model.browse.importReconciliation {
                return .red
            }
            return model.browse.importReconciliation == .landed ? .green : .blue
        case .failed:
            return .red
        case .idle:
            return .green
        }
    }

    private var terminalRefreshToken: String? {
        switch imports.phase {
        case .completed(let outcome):
            return "\(imports.operationID ?? "operation")-completed-\(outcome.copiedCount)"
        case .cancelled(let copiedCount):
            return "\(imports.operationID ?? "operation")-cancelled-\(copiedCount)"
        default:
            return nil
        }
    }

    private func syncImportPresentation(for catalog: RecentCatalog) async {
        guard imports.catalog?.path == catalog.path else {
            model.browse.clearImportPresentation()
            return
        }
        await model.browse.updateImportPresentation(
            operationID: imports.operationID,
            collectionName: imports.collectionName,
            media: imports.verifiedMedia
        )
    }

    @ViewBuilder
    private var importActivity: some View {
        switch imports.phase {
        case .running, .recovering:
            importProgress
        case .stopping:
            activityBanner(
                icon: "stop.circle",
                title: "Stopping safely…",
                message: "Souvenirs keeps every verified item and records where the import stopped.",
                tint: .secondary,
                identifier: "import-stopping",
                actionTitle: nil
            )
        case .completed(let outcome):
            terminalActivity(
                success: completedImport(outcome),
                pendingMessage: "Souvenirs is confirming the imported media in the archive."
            )
        case .cancelled(let copiedCount):
            terminalActivity(
                success: activityBanner(
                    icon: "stop.circle.fill",
                    title: "Import stopped",
                    message: "\(copiedCount) verified items remain in your archive. Nothing on the source was changed.",
                    tint: .secondary,
                    identifier: "import-cancelled",
                    actionTitle: "Done",
                    action: imports.dismissOutcome
                ),
                pendingMessage: "Souvenirs is confirming the admitted partial import."
            )
        case .failed(let message):
            activityBanner(
                icon: "exclamationmark.triangle.fill",
                title: "Import needs attention",
                message: message,
                tint: .red,
                identifier: "import-failed",
                actionTitle: "Dismiss",
                action: imports.dismissOutcome
            )
        case .idle, .recoveryPrompt, .recoveryDeferred:
            Spacer()
        }
    }

    private var importProgress: some View {
        VStack(alignment: .leading, spacing: 9) {
            HStack {
                Text(
                    imports.progressMessage == "Checking the source…"
                        ? "Checking the source"
                        : "Copying \(imports.completedUnits) of \(imports.totalUnits)"
                )
                    .font(.headline)
                    .accessibilityIdentifier("import-progress-count")
                Spacer()
                Text("\(imports.percentCompleted)%")
                    .foregroundStyle(.secondary)
            }
            ProgressView(value: imports.fractionCompleted)
                .progressViewStyle(.linear)
                .accessibilityIdentifier("import-progress-bar")
            HStack {
                Label(
                    "Keep \(imports.sourceName) connected. Nothing on it will change. You can close this window.",
                    systemImage: "checkmark.shield"
                )
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Stop…") { imports.stop() }
                    .disabled(!imports.canStop)
                    .accessibilityIdentifier("stop-import")
                    .focusable()
                    .focused($keyboardFocus, equals: .stopImport)
                    .accessibilityFocused(
                        $accessibilityFocus,
                        equals: .stopImport
                    )
            }
            Text(imports.progressMessage)
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityIdentifier("import-progress-message")
        }
        .padding(14)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
        .shadow(color: .black.opacity(0.08), radius: 10, y: 4)
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    private func terminalActivity<Success: View>(
        success: Success,
        pendingMessage: String
    ) -> some View {
        Group {
            switch model.browse.importReconciliation {
            case .landed:
                success
            case .failed(let message):
                activityBanner(
                    icon: "externaldrive.badge.exclamationmark",
                    title: "Archive access needs attention",
                    message: message,
                    tint: .red,
                    identifier: "import-terminal-archive-error",
                    actionTitle: "Try Again",
                    action: {
                        Task { await model.browse.reconcileTerminalImport() }
                    },
                    secondaryActionTitle: "Continue Browsing",
                    secondaryAction: {
                        model.browse.clearImportPresentation()
                        imports.dismissOutcome()
                    }
                )
            case .active, .refreshing, .none:
                activityBanner(
                    icon: "arrow.trianglehead.2.clockwise.rotate.90",
                    title: "Updating the archive…",
                    message: pendingMessage,
                    tint: .blue,
                    identifier: "import-terminal-refreshing",
                    actionTitle: nil
                )
            }
        }
    }

    private func completedImport(_ outcome: ImportOutcome) -> some View {
        HStack(spacing: 11) {
            Image(systemName: "checkmark.circle.fill")
                .font(.title2)
                .foregroundStyle(.green)
            VStack(alignment: .leading, spacing: 2) {
                Text("All \(outcome.copiedCount) copied and checked. You can eject \(imports.sourceName).")
                    .font(.headline)
                    .accessibilityIdentifier("import-completed")
                Text("\(outcome.skippedCount) already in your archive were skipped · \(outcome.leftOnSourceCount) remain on the source")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            Button("Done") { imports.dismissOutcome() }
                .accessibilityIdentifier("dismiss-import-completed")
        }
        .padding(14)
        .background(.green.opacity(0.09), in: RoundedRectangle(cornerRadius: 12))
        .overlay {
            RoundedRectangle(cornerRadius: 12).stroke(.green.opacity(0.28))
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
    }

    private var deferredRecovery: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "externaldrive.badge.exclamationmark")
                    .font(.title2)
                    .foregroundStyle(.orange)
                VStack(alignment: .leading, spacing: 3) {
                    Text("Import paused")
                        .font(.headline)
                        .accessibilityIdentifier("import-recovery-deferred")
                    Text(
                        "\(imports.completedUnits) verified items are safe. Choose Try Again when you're ready to finish importing “\(imports.collectionName)”."
                    )
                    .font(.caption)
                    .foregroundStyle(.secondary)
                }
            }
            HStack {
                Spacer()
                Button("Stop Here…") { imports.stopRecoveryHere() }
                    .accessibilityIdentifier("cancel-deferred-recovery")
                Button("Try Again") {
                    modalReturnFocus = .retryDeferredRecovery
                    imports.retryRecovery()
                }
                    .buttonStyle(.borderedProminent)
                    .accessibilityIdentifier("retry-deferred-recovery")
                    .focusable()
                    .focused($keyboardFocus, equals: .retryDeferredRecovery)
                    .accessibilityFocused(
                        $accessibilityFocus,
                        equals: .retryDeferredRecovery
                    )
            }
        }
        .padding(12)
        .background(.orange.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
        .overlay {
            RoundedRectangle(cornerRadius: 10).stroke(.orange.opacity(0.3))
        }
        .padding(.horizontal, 18)
    }

    private func activityBanner(
        icon: String,
        title: String,
        message: String,
        tint: Color,
        identifier: String,
        actionTitle: String?,
        action: @escaping () -> Void = {},
        secondaryActionTitle: String? = nil,
        secondaryAction: @escaping () -> Void = {}
    ) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: icon)
                .font(.title2)
                .foregroundStyle(tint)
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.headline)
                    .accessibilityIdentifier(identifier)
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if let secondaryActionTitle {
                Button(secondaryActionTitle, action: secondaryAction)
                    .accessibilityIdentifier("\(identifier)-secondary-action")
            }
            if let actionTitle {
                Button(actionTitle, action: action)
                    .accessibilityIdentifier("\(identifier)-action")
            }
        }
        .padding(14)
        .background(tint.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
        .overlay {
            RoundedRectangle(cornerRadius: 12).stroke(tint.opacity(0.24))
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 12)
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
            .padding(.horizontal, 10)
            .frame(height: 50)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!engineStatus.catalogActionsAvailable)
        .foregroundStyle(prominent ? Color.white : Color.primary)
        .background(
            prominent ? Color.accentColor : Color(nsColor: .controlColor),
            in: RoundedRectangle(cornerRadius: 10)
        )
        .opacity(engineStatus.catalogActionsAvailable ? 1 : 0.62)
        .accessibilityHint(catalogActionAccessibilityHint)
        .accessibilityIdentifier(identifier)
    }

    @ViewBuilder
    private var catalogActionsUnavailableNotice: some View {
        if let description = engineStatus.catalogActionsUnavailableDescription {
            VStack(alignment: .leading, spacing: 7) {
                Label("Archive actions unavailable", systemImage: "exclamationmark.triangle")
                    .font(.caption.weight(.medium))
                Text(description)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                if case .failed = engineStatus, let onRetryEngine {
                    Button("Try Again", action: onRetryEngine)
                        .accessibilityIdentifier("retry-engine")
                }
            }
            .padding(9)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.yellow.opacity(0.09), in: RoundedRectangle(cornerRadius: 10))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Archive actions unavailable")
            .accessibilityValue(description)
            .accessibilityIdentifier("catalog-actions-unavailable")
        }
    }

    private var catalogActionAccessibilityHint: String {
        engineStatus.catalogActionsUnavailableDescription ?? ""
    }

    private var engineBadge: some View {
        Group {
            switch engineStatus {
            case .connecting:
                Label("Connecting…", systemImage: "ellipsis.circle")
            case .ready:
                Label("Engine ready", systemImage: "checkmark.circle.fill")
            case .failed:
                Label("Engine unavailable", systemImage: "exclamationmark.triangle.fill")
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

struct CatalogCreationSheet: View {
    private enum FocusTarget: Hashable {
        case name
    }

    @ObservedObject var model: CatalogWelcomeModel
    let onChooseLocation: () -> Void
    let onCancel: () -> Void
    let onCreate: () -> Void
    @State private var showsCreatedItems = false
    @FocusState private var keyboardFocus: FocusTarget?
    @AccessibilityFocusState private var titleFocused: Bool

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 15) {
                    Text("New Archive")
                        .font(.title2.bold())
                        .accessibilityIdentifier("catalog-create-title")
                        .accessibilityFocused($titleFocused)
                    Text("Choose where your photos will live. You can open this folder in Finder at any time.")
                        .font(.callout)
                        .foregroundStyle(.secondary)

                    VStack(alignment: .leading, spacing: 6) {
                        Text("Name").font(.caption)
                        TextField("Archive name", text: $model.catalogName)
                            .textFieldStyle(.roundedBorder)
                            .accessibilityIdentifier("catalog-name")
                            .focused($keyboardFocus, equals: .name)
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

                            Button("Choose…", action: onChooseLocation)
                                .accessibilityIdentifier("choose-catalog-location")
                        }
                        validationLine
                    }

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

                    VStack(alignment: .leading, spacing: 4) {
                        Button {
                            showsCreatedItems.toggle()
                        } label: {
                            HStack(spacing: 6) {
                                Image(systemName: "chevron.right")
                                    .font(.caption2)
                                    .rotationEffect(.degrees(showsCreatedItems ? 90 : 0))
                                Text("What will be created")
                                Spacer()
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("created-items-disclosure")
                        .accessibilityValue(showsCreatedItems ? "Expanded" : "Collapsed")

                        if showsCreatedItems {
                            Text(".souvenirs.yml and a small .souvenirs folder. Your photos are added later, by import.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .padding(.top, 4)
                                .accessibilityIdentifier("created-items-description")
                        }
                    }
                    .padding(12)
                    .background(.quaternary.opacity(0.55), in: RoundedRectangle(cornerRadius: 10))
                }
                .padding(24)
            }

            Divider()
            HStack {
                Spacer()
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("cancel-create-catalog")
                Button("Create Archive", action: onCreate)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!model.canCreate)
                    .accessibilityIdentifier("confirm-create-catalog")
            }
            .padding(16)
            .background(.regularMaterial)
        }
        .onAppear {
            keyboardFocus = .name
            titleFocused = true
        }
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
            Label(
                "This location works. Other Macs can open it too, one at a time.",
                systemImage: "checkmark"
            )
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
}

private struct ImportPlanningSheet: View {
    private enum FocusTarget: Hashable {
        case cancel
        case retry
        case collectionName
        case destination
        case dateSummary
        case asShown
        case convertFromUTC
    }

    @ObservedObject var model: ImportPlanningModel
    let onCancel: () -> Void
    let onStart: () -> Void
    @FocusState private var keyboardFocus: FocusTarget?
    @AccessibilityFocusState private var titleFocused: Bool
    @AccessibilityFocusState private var destinationFocused: Bool
    @AccessibilityFocusState private var dateFocused: FocusTarget?
    @AccessibilityFocusState private var recoveryFocused: FocusTarget?
    @State private var requestedDateFocus: FocusTarget?
    @State private var requestedRecoveryFocus: FocusTarget?
    @State private var showsExistingCollections = false

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Text("Import from \(model.scan?.sourceName ?? "source")")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("import-review-sheet")
                    Text(model.title)
                        .font(.system(size: 22, weight: .bold))
                        .accessibilityIdentifier("import-review-title")
                        .accessibilityFocused($titleFocused)

                    if let scan = model.scan {
                        destinationCard
                        if model.dateChoiceCount > 0 {
                            dateDecision
                        }
                        summary(scan)
                        details
                    } else if model.isLoading {
                        HStack(spacing: 10) {
                            ProgressView()
                            Text("Scanning without changing the source…")
                                .foregroundStyle(.secondary)
                        }
                        .frame(maxWidth: .infinity, minHeight: 160)
                        .accessibilityIdentifier("import-scan-progress")
                    }

                    if let message = model.errorMessage {
                        VStack(alignment: .leading, spacing: 10) {
                            Label(message, systemImage: "exclamationmark.triangle.fill")
                                .font(.callout)
                                .foregroundStyle(.red)
                                .accessibilityLabel(
                                    model.canRetryScan
                                        ? "Import source unavailable. \(message)"
                                        : message
                                )
                                .accessibilityIdentifier("import-planning-error")
                            if model.canRetryScan {
                                Button("Try Again") {
                                    requestedRecoveryFocus = .collectionName
                                    Task { await model.retryScan() }
                                }
                                .buttonStyle(.borderedProminent)
                                .accessibilityHint(
                                    "Scans the same source again without closing this sheet."
                                )
                                .accessibilityIdentifier("retry-import-scan")
                                .focusable()
                                .focused($keyboardFocus, equals: .retry)
                                .accessibilityFocused($recoveryFocused, equals: .retry)
                                .task {
                                    requestedRecoveryFocus = nil
                                    await Task.yield()
                                    guard model.canRetryScan else { return }
                                    keyboardFocus = .retry
                                    recoveryFocused = .retry
                                }
                            }
                        }
                        .padding(12)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(.red.opacity(0.07), in: RoundedRectangle(cornerRadius: 9))
                    }
                }
                .padding(24)
            }

            Divider()
            HStack(spacing: 8) {
                Label(
                    "Nothing in the source will be changed or deleted.",
                    systemImage: "checkmark.shield"
                )
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .layoutPriority(-1)
                Spacer(minLength: 0)
                if model.isStartBlocked {
                    Text("Can’t start")
                        .font(.caption.bold())
                        .foregroundStyle(.red)
                        .accessibilityIdentifier("import-start-blocked")
                }
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("cancel-import-review")
                    .focused($keyboardFocus, equals: .cancel)
                Button(model.isPrepared ? "Plan Ready" : "Start Import") {
                    model.prepareExecution()
                    if model.isPrepared { onStart() }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(!model.canStart || model.isPrepared)
                .accessibilityIdentifier("start-import")
                .accessibilityHint(model.startImportAccessibilityHint)
            }
            .padding(16)
            .background(.regularMaterial)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onAppear {
            if model.canRetryScan {
                keyboardFocus = .retry
                recoveryFocused = .retry
            } else {
                keyboardFocus = model.scan == nil ? .cancel : .collectionName
                titleFocused = true
            }
        }
        .onChange(of: model.canRetryScan) {
            if $1 {
                keyboardFocus = .retry
            }
        }
        .onChange(of: model.scan != nil) {
            if $1 {
                keyboardFocus = .collectionName
            }
        }
        .sheet(
            isPresented: $showsExistingCollections,
            onDismiss: {
                Task { @MainActor in
                    try? await Task.sleep(for: .milliseconds(150))
                    guard !showsExistingCollections else { return }
                    keyboardFocus = .destination
                    destinationFocused = true
                }
            }
        ) {
            ExistingCollectionSelector(
                collections: model.availableCollections,
                selectedIdentity: model.selectedExistingCollection?.identity,
                isLoading: model.isLoadingCollections,
                errorMessage: model.collectionSelectionError,
                onCancel: {
                    showsExistingCollections = false
                },
                onSelect: { collection in
                    showsExistingCollections = false
                    Task { await model.selectExistingCollection(collection) }
                }
            )
        }
    }

    private var destinationCard: some View {
        HStack(spacing: 12) {
            thumbnailGrid
            VStack(alignment: .leading, spacing: 5) {
                Text(
                    model.destinationMode == .create
                        ? "Collection name" : "Selected collection"
                )
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("import-destination-summary")
                HStack {
                    if model.destinationMode == .create {
                        TextField("Collection name", text: $model.collectionName)
                            .textFieldStyle(.roundedBorder)
                            .onChange(of: model.collectionName) {
                                model.collectionNameChanged()
                            }
                            .onSubmit { Task { await model.refreshReview() } }
                            .accessibilityIdentifier("import-collection-name")
                            .focused($keyboardFocus, equals: .collectionName)
                            .accessibilityFocused($recoveryFocused, equals: .collectionName)
                            .onAppear {
                                guard requestedRecoveryFocus == .collectionName else { return }
                                requestedRecoveryFocus = nil
                                keyboardFocus = .collectionName
                                recoveryFocused = .collectionName
                            }
                    } else {
                        Text(model.collectionName)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 8)
                            .frame(height: 22)
                            .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 5))
                            .accessibilityIdentifier("import-collection-name")
                            .accessibilityLabel("Selected collection")
                            .accessibilityValue(model.collectionName)
                    }
                    Button(
                        model.destinationMode == .create
                            ? "Add to Existing…" : "Change…"
                    ) {
                        showsExistingCollections = true
                        Task { await model.loadCollections() }
                    }
                    .accessibilityIdentifier("import-destination-mode")
                    .focusable()
                    .focused($keyboardFocus, equals: .destination)
                    .accessibilityFocused($destinationFocused)
                    if model.destinationMode == .existing {
                        Button("Create New…") {
                            keyboardFocus = .destination
                            destinationFocused = true
                            Task { await model.useCreateDestination() }
                        }
                        .accessibilityIdentifier("import-create-new-destination")
                    }
                }
                Text(dateSummary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

        }
        .padding(12)
        .background(.background.opacity(0.75), in: RoundedRectangle(cornerRadius: 11))
        .overlay {
            RoundedRectangle(cornerRadius: 11).stroke(.separator.opacity(0.55))
        }
    }

    private var dateDecision: some View {
        Group {
            if let resolution = model.dateResolution {
                HStack(spacing: 10) {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(.orange)
                        .accessibilityHidden(true)
                    Text(selectedDateSummary(resolution))
                        .font(.callout.bold())
                        .focusable()
                        .focused($keyboardFocus, equals: .dateSummary)
                        .accessibilityFocused($dateFocused, equals: .dateSummary)
                        .accessibilityIdentifier("import-date-selection")
                        .accessibilityLabel("Selected date interpretation")
                        .accessibilityValue(selectedDateSummary(resolution))
                        .onAppear {
                            guard requestedDateFocus == .dateSummary else { return }
                            requestedDateFocus = nil
                            keyboardFocus = .dateSummary
                            dateFocused = .dateSummary
                        }
                    Spacer()
                    Button("Change") {
                        Task {
                            requestedDateFocus = .asShown
                            await model.changeDateResolution()
                            if model.dateResolution != nil {
                                requestedDateFocus = nil
                            }
                        }
                    }
                    .accessibilityIdentifier("change-date-selection")
                    .accessibilityHint("Shows the date interpretation options again.")
                }
            } else {
                VStack(alignment: .leading, spacing: 8) {
                    Text(
                        "Optional date choice · \(itemCount(model.dateChoiceCount))"
                    )
                        .font(.caption.bold())
                        .foregroundStyle(.orange)
                        .accessibilityIdentifier("import-date-decision")
                    HStack(alignment: .top, spacing: 10) {
                        Image(systemName: "questionmark")
                            .font(.caption.bold())
                            .foregroundStyle(.orange)
                            .frame(width: 22, height: 22)
                            .background(.orange.opacity(0.15), in: Circle())
                        VStack(alignment: .leading, spacing: 7) {
                            ForEach(
                                Array(model.interpretationChoiceSources.enumerated()),
                                id: \.offset
                            ) { _, summary in
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(pendingDateSummary(summary))
                                        .font(.callout.bold())
                                    if let asShownTime = summary.asShownTime {
                                        Text("Example · As shown: \(asShownTime)")
                                            .font(.caption2)
                                            .foregroundStyle(.secondary)
                                    }
                                    if summary.supportsUTCConversion,
                                       let convertedTime = summary.convertedFromUTCTime {
                                        Text("Example · Convert from UTC: \(convertedTime)")
                                            .font(.caption2)
                                            .foregroundStyle(.secondary)
                                    }
                                }
                            }
                            Text(dateFallbackDescription)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            HStack {
                                decisionButton(
                                    model.supportsUTCConversion ? "As shown" : "Use proposed dates",
                                    value: .asShown,
                                    focus: .asShown
                                )
                                if model.supportsUTCConversion {
                                    decisionButton(
                                        "Convert from UTC",
                                        detail: "Applies current \(model.utcOffsetDescription) to ambiguous items",
                                        value: .convertFromUTC,
                                        focus: .convertFromUTC
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
        .padding(12)
        .background(.orange.opacity(0.035), in: RoundedRectangle(cornerRadius: 11))
        .overlay {
            RoundedRectangle(cornerRadius: 11)
                .stroke(.orange, lineWidth: model.dateResolution == nil ? 1.2 : 0.6)
        }
    }

    private func decisionButton(
        _ title: String,
        detail: String? = nil,
        value: EngineSupervisor.ImportDateResolution,
        focus: FocusTarget
    ) -> some View {
        Button {
            Task {
                requestedDateFocus = .dateSummary
                await model.resolveDate(value)
                if model.dateResolution == nil {
                    requestedDateFocus = nil
                }
            }
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                if let detail {
                    Text(detail)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .buttonStyle(.bordered)
        .tint(.secondary)
        .accessibilityLabel(
            [
                title,
                detail
            ]
            .compactMap { $0 }
            .joined(separator: ", ")
        )
        .accessibilityIdentifier(
            value == .asShown ? "date-as-shown" : "date-convert-utc"
        )
        .focused($keyboardFocus, equals: focus)
        .accessibilityFocused($dateFocused, equals: focus)
        .onAppear {
            guard focus == .asShown, requestedDateFocus == .asShown else { return }
            requestedDateFocus = nil
            keyboardFocus = .asShown
            dateFocused = .asShown
        }
    }

    private var dateFallbackDescription: String {
        if model.dateChoicePreventsStart {
            return """
                These items will stay on the source until you select an interpretation.
                """
        }
        if !model.canStart {
            return """
                These items will be skipped, stay on the source, and remain available for a \
                later import unless you select an interpretation.
                """
        }
        return """
            You can start now. These items will be skipped, stay on the source, and remain \
            available for a later import unless you select an interpretation.
            """
    }

    private func selectedDateSummary(
        _ resolution: EngineSupervisor.ImportDateResolution
    ) -> String {
        switch resolution {
        case .asShown:
            return model.supportsUTCConversion ? "As shown" : "Proposed dates"
        case .convertFromUTC:
            return "Converted from UTC where applicable"
        }
    }

    private func summary(_ scan: EngineSupervisor.ImportScan) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            Label(
                "\(model.review?.transferCount ?? scan.mediaCount) photos and videos will be copied.",
                systemImage: "square.and.arrow.down"
            )
            .accessibilityIdentifier("import-operation-summary")
            if scan.unsupportedCount > 0 {
                Label(
                    "\(scan.unsupportedCount) unsupported files will stay on the source.",
                    systemImage: "externaldrive"
                )
            }
            if scan.unresolvedItemCount > 0 {
                Label(
                    "\(scan.unresolvedItemCount) items have no usable date and will stay on the source.",
                    systemImage: "calendar.badge.exclamationmark"
                )
            }
            if model.deferredDateCount > 0 {
                Label(
                    "\(itemCount(model.deferredDateCount)) \(model.deferredDateCount == 1 ? "needs" : "need") a time and will stay on the source.",
                    systemImage: "clock.badge.exclamationmark"
                )
            }
            if let review = model.review {
                let unappliedDateCount = max(
                    0,
                    review.skippedCount - model.deferredDateCount
                )
                if model.dateResolution != nil, unappliedDateCount > 0 {
                    Label(
                        "\(itemCount(unappliedDateCount)) could not use the selected interpretation and will stay on the source.",
                        systemImage: "calendar.badge.exclamationmark"
                    )
                }
                Label(
                    "\(review.operationCount) verified operations · \(bytes(review.preflight.requiredBytes))",
                    systemImage: "checkmark.circle"
                )
            }

        }
        .font(.caption)
        .foregroundStyle(.secondary)
    }

    private var dateSummary: String {
        guard let start = model.scan?.suggestedAcceptedDate else {
            return "No accepted media date was found"
        }
        let range = model.scan?.suggestedRangeEnd.flatMap {
            $0 == start ? nil : "\(start)–\($0)"
        } ?? start
        let sources = model.scan?.datetimeSources
            .filter { $0.state == .accepted }
            .map { dateSourceName($0.source) }
            .joined(separator: ", ") ?? ""
        return sources.isEmpty ? range : "\(range) · \(sources)"
    }

    private func pendingDateSummary(
        _ summary: EngineSupervisor.ImportDateSourceSummary
    ) -> String {
        switch summary.source {
        case .filenamePattern:
            return "\(summary.itemCount) filename date proposals"
        case .containerCreationTime:
            return "\(summary.itemCount) ambiguous container dates"
        default:
            return "\(summary.itemCount) \(dateSourceName(summary.source)) dates"
        }
    }

    private func dateSourceName(_ source: EngineSupervisor.ImportDateSource) -> String {
        switch source {
        case .exifDateTimeOriginal: "EXIF DateTimeOriginal"
        case .exifDateTimeDigitized: "EXIF DateTimeDigitized"
        case .quickTimeCreationDate: "QuickTime creation date"
        case .xmpDateCreated: "owned XMP DateCreated"
        case .xmpExifDateTimeOriginal: "owned XMP DateTimeOriginal"
        case .containerCreationTime: "container creation time"
        case .googleTakeoutPhotoTakenTime: "Google Takeout proposal"
        case .filenamePattern: "filename proposal"
        }
    }

    private var details: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                model.showsDetails.toggle()
            } label: {
                HStack {
                    Image(systemName: model.showsDetails ? "chevron.down" : "chevron.right")
                        .accessibilityHidden(true)
                    Text("Details: destination and checks")
                    Spacer()
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("import-details")
            .accessibilityValue(model.showsDetails ? "Expanded" : "Collapsed")

            if model.showsDetails, let review = model.review {
                VStack(alignment: .leading, spacing: 12) {
                    detailSection(title: "Where the files go") {
                        Text(review.destinationPath)
                            .textSelection(.enabled)
                            .accessibilityElement(children: .ignore)
                            .accessibilityLabel(review.destinationPath)
                            .accessibilityIdentifier("import-detail-destination")
                        if !review.preflight.representativeFileNames.isEmpty {
                            Text("Example file names")
                                .font(.caption2.bold())
                                .foregroundStyle(.secondary)
                            ForEach(
                                Array(review.preflight.representativeFileNames.enumerated()),
                                id: \.offset
                            ) { index, fileName in
                                Text(fileName)
                                    .textSelection(.enabled)
                                    .accessibilityElement(children: .ignore)
                                    .accessibilityLabel(fileName)
                                    .accessibilityIdentifier("import-detail-file-\(index)")
                            }
                        }
                    }

                    if hasCompletedPreflight(review.preflight) {
                        detailSection(title: "Checks completed during review") {
                            if let availableBytes = review.preflight.availableBytes {
                                if review.preflight.availableSpace == .passed {
                                    detailCheck(
                                        "\(bytes(review.preflight.requiredBytes)) needed; "
                                            + "\(bytes(availableBytes)) available when reviewed",
                                        identifier: "import-detail-space"
                                    )
                                } else if review.preflight.availableSpace == .failed {
                                    Label(
                                        "\(bytes(review.preflight.requiredBytes)) needed; "
                                            + "only \(bytes(availableBytes)) available when reviewed",
                                        systemImage: "exclamationmark.triangle"
                                    )
                                    .foregroundStyle(.orange)
                                    .accessibilityIdentifier("import-detail-space")
                                }
                                Text(
                                    "Available space is observed during review "
                                        + "and can change before import."
                                )
                            }
                            if review.preflight.writeProbe == .passed {
                                detailCheck(
                                    "The archive accepted a confirmed test write",
                                    identifier: "import-detail-write-probe"
                                )
                            }
                            if review.preflight.portableNames == .passed {
                                detailCheck(
                                    "Every planned name works on supported storage",
                                    identifier: "import-detail-portable-names"
                                )
                            }
                            if review.preflight.destinationConflicts == .passed {
                                detailCheck(
                                    "The planned names do not replace existing files",
                                    identifier: "import-detail-no-replacement"
                                )
                            }
                        }
                    }

                    detailSection(title: "How each file is verified") {
                        Text("1. Copy it to a hidden working folder.")
                        Text("2. Read the copy back and compare it with the source.")
                        Text("3. Publish the final name and checksum.")
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("import-detail-copy-sequence")

                    Text(
                        "If the import is interrupted, Souvenirs keeps enough information "
                            + "to resume or cancel safely when you reopen the archive."
                    )
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("import-detail-recovery")
                }
                .padding(.top, 8)
                .textSelection(.enabled)
            }
        }
    }

    private func detailSection<Content: View>(
        title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title).font(.caption.bold())
            content()
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .background(.background.opacity(0.7), in: RoundedRectangle(cornerRadius: 9))
    }

    private func detailCheck(_ text: String, identifier: String) -> some View {
        Label(text, systemImage: "checkmark")
            .accessibilityIdentifier(identifier)
    }

    private func hasCompletedPreflight(
        _ evidence: EngineSupervisor.ImportPreflightEvidence
    ) -> Bool {
        evidence.availableBytes != nil
            || evidence.availableSpace != .notRun
            || evidence.writeProbe == .passed
            || evidence.portableNames == .passed
            || evidence.destinationConflicts == .passed
    }

    private var thumbnailGrid: some View {
        LazyVGrid(columns: [GridItem(.fixed(40)), GridItem(.fixed(40))], spacing: 2) {
            Color.blue.opacity(0.45)
            Color.teal.opacity(0.35)
            Color.green.opacity(0.5)
            Color.indigo.opacity(0.45)
        }
        .frame(width: 82, height: 82)
        .clipShape(RoundedRectangle(cornerRadius: 6))
        .accessibilityHidden(true)
    }

    private func bytes(_ count: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(count), countStyle: .file)
    }

    private func itemCount(_ count: Int) -> String {
        "\(count) \(count == 1 ? "item" : "items")"
    }
}

private struct ExistingCollectionSelector: View {
    let collections: [EngineSupervisor.CatalogCollection]
    let selectedIdentity: EngineSupervisor.CatalogCollectionIdentity?
    let isLoading: Bool
    let errorMessage: String?
    let onCancel: () -> Void
    let onSelect: (EngineSupervisor.CatalogCollection) -> Void

    @State private var selection: EngineSupervisor.CatalogCollectionIdentity?
    @FocusState private var keyboardFocus: EngineSupervisor.CatalogCollectionIdentity?
    @AccessibilityFocusState private var accessibilityFocus:
        EngineSupervisor.CatalogCollectionIdentity?

    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 8) {
                Text("Add to an Existing Collection")
                    .font(.system(size: 20, weight: .bold))
                    .accessibilityIdentifier("existing-collection-selector")
                Text("Choose the exact collection for this import.")
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(20)

            Divider()

            Group {
                if isLoading {
                    ProgressView("Reading collections…")
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .accessibilityIdentifier("existing-collections-loading")
                } else if let errorMessage {
                    Label(errorMessage, systemImage: "exclamationmark.triangle.fill")
                        .foregroundStyle(.red)
                        .padding(20)
                        .frame(
                            maxWidth: .infinity,
                            maxHeight: .infinity,
                            alignment: .topLeading
                        )
                        .accessibilityIdentifier("existing-collections-error")
                } else if collections.isEmpty {
                    ContentUnavailableView(
                        "No Collections",
                        systemImage: "photo.on.rectangle.angled",
                        description: Text("Create a collection before adding to an existing one.")
                    )
                    .accessibilityIdentifier("existing-collections-empty")
                } else {
                    List(
                        Array(collections.enumerated()),
                        id: \.element.identity
                    ) { index, collection in
                        collectionRow(collection, index: index)
                    }
                }
            }
            .frame(minHeight: 260)

            Divider()
            HStack {
                Button("Cancel", action: onCancel)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("cancel-existing-collection")
                Spacer()
                Button("Add to This Collection") {
                    guard let collection = collections.first(where: {
                        $0.identity == selection
                    }) else { return }
                    onSelect(collection)
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(
                    !collections.contains(where: { $0.identity == selection })
                        || isLoading || errorMessage != nil
                )
                .accessibilityIdentifier("confirm-existing-collection")
            }
            .padding(16)
        }
        .frame(width: 520, height: 430)
        .onAppear {
            focus(selectedIdentity ?? collections.first?.identity)
        }
        .onChange(of: collections) {
            guard selection == nil || !$1.contains(where: { $0.identity == selection })
            else { return }
            focus($1.first?.identity)
        }
    }

    private func collectionRow(
        _ collection: EngineSupervisor.CatalogCollection,
        index: Int
    ) -> some View {
        Button {
            selection = collection.identity
        } label: {
            HStack(spacing: 10) {
                Image(
                    systemName: selection == collection.identity
                        ? "checkmark.circle.fill" : "circle"
                )
                .foregroundStyle(
                    selection == collection.identity
                        ? Color.accentColor : Color.secondary
                )
                .accessibilityHidden(true)
                VStack(alignment: .leading, spacing: 3) {
                    Text(collection.label)
                        .font(.body.bold())
                    Text(detail(for: collection))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("existing-collection-\(index)")
        .accessibilityLabel(collection.label)
        .accessibilityValue(detail(for: collection))
        .accessibilityAddTraits(
            selection == collection.identity ? .isSelected : []
        )
        .focused($keyboardFocus, equals: collection.identity)
        .accessibilityFocused($accessibilityFocus, equals: collection.identity)
    }

    private func focus(_ identity: EngineSupervisor.CatalogCollectionIdentity?) {
        selection = identity
        keyboardFocus = identity
        accessibilityFocus = identity
    }

    private func detail(for collection: EngineSupervisor.CatalogCollection) -> String {
        switch collection {
        case .dated(_, _, let startDate, let endDate):
            return startDate == endDate ? startDate : "\(startDate) through \(endDate)"
        case .ongoing(_, _, let parts):
            let years = parts.map(\.year).map(String.init).joined(separator: ", ")
            return "Ongoing · \(years)"
        }
    }
}

private struct RecoverySheet: View {
    private enum FocusTarget: Hashable {
        case finish
        case retry
    }

    @ObservedObject var imports: ImportExecutionCoordinator
    let sourceAvailable: Bool
    let onStop: () -> Void
    let onDefer: () -> Void
    let onRetry: () -> Void
    let onFinish: () -> Void
    @FocusState private var keyboardFocus: FocusTarget?
    @AccessibilityFocusState private var titleFocused: Bool
    @State private var recoveryChecksExpanded = false

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    Label(
                        sourceAvailable
                            ? imports.remainingUnits == 0
                                ? "Finish recovery for “\(imports.collectionName)”?"
                                : "Resume importing “\(imports.collectionName)”?"
                            : "Connect \(imports.sourceName) to finish the import",
                        systemImage: sourceAvailable ? "arrow.clockwise" : "externaldrive"
                    )
                    .font(.title3.bold())
                    .accessibilityIdentifier("recovery-title")
                    .accessibilityFocused($titleFocused)
                    Text(interruptionSummary)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("recovery-interruption")
                    if let activityTime = imports.recoveryActivityTime {
                    LabeledContent(
                        activityTime.kind == .interruptedAt
                            ? "Interrupted at"
                            : "Last recorded activity",
                        value: activityTime.date.formatted(
                            date: .abbreviated,
                            time: .shortened
                        )
                    )
                    .accessibilityIdentifier("recovery-activity-time")
                    }
                    LabeledContent(
                    "Source",
                    value: "\(imports.sourceName) · \(sourceAvailable ? "Connected" : "Not connected")"
                    )
                    .accessibilityIdentifier("recovery-source-status")
                    Text(recoveryCountSummary)
                    .accessibilityIdentifier("recovery-counts")
                    Text(
                    "Stop Here… keeps the copied items and records the remaining items as not imported."
                    )
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("recovery-stop-explanation")
                    if !sourceAvailable {
                    Label(
                        "Reconnect \(imports.sourceName), then choose Try Again to resume recovery.",
                        systemImage: "externaldrive.badge.exclamationmark"
                        )
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("recovery-source-unavailable")
                    } else {
                        DisclosureGroup(
                            "Recovery checks",
                            isExpanded: $recoveryChecksExpanded
                        ) {
                            VStack(alignment: .leading, spacing: 8) {
                                Label(
                                    "Confirm copied items are complete.",
                                    systemImage: "checkmark.circle"
                                )
                                Label(
                                    "Verify the source before copying the remaining items.",
                                    systemImage: "checkmark.circle"
                                )
                            }
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .padding(.top, 6)
                        }
                        .accessibilityIdentifier("recovery-checks")
                    }
                }
                .padding(24)
            }
            Divider()
            HStack {
                Button("Stop Here…", action: onStop)
                    .disabled(imports.isRetryingRecovery)
                    .accessibilityIdentifier("cancel-recovery")
                Spacer()
                if imports.isRetryingRecovery {
                    ProgressView()
                        .controlSize(.small)
                    Text("Checking source…")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("recovery-retry-progress")
                }
                Button("Not Now", action: onDefer)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("defer-recovery")
                if !sourceAvailable {
                    Button("Try Again", action: onRetry)
                        .disabled(imports.isRetryingRecovery)
                        .accessibilityIdentifier("retry-recovery")
                        .focused($keyboardFocus, equals: .retry)
                }
                Button("Resume Import", action: onFinish)
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(!sourceAvailable || imports.isRetryingRecovery)
                    .accessibilityIdentifier("finish-recovery")
                    .focused($keyboardFocus, equals: .finish)
            }
            .padding(16)
            .background(.regularMaterial)
        }
        .onAppear {
            keyboardFocus = sourceAvailable ? .finish : .retry
            titleFocused = true
        }
    }

    private var interruptionSummary: String {
        switch imports.interruption {
        case .sourceUnavailable:
            return "The source became unavailable before the import finished. Nothing was lost."
        case .incompleteWithoutTerminalRecord, nil:
            return "The import stopped before it finished. Nothing was lost."
        }
    }

    private func itemCount(_ count: UInt64) -> String {
        "\(count) \(count == 1 ? "item" : "items")"
    }

    private var recoveryCountSummary: String {
        let safe = "\(itemCount(imports.completedUnits)) "
            + "\(imports.completedUnits == 1 ? "is" : "are") safely in your archive."
        if imports.remainingUnits == 0 {
            return "\(safe) 0 items remain to copy from \(imports.sourceName). "
                + "Recovery still needs to finish the archive record."
        }
        return "\(safe) \(itemCount(imports.remainingUnits)) "
            + "\(imports.remainingUnits == 1 ? "remains" : "remain") "
            + "on \(imports.sourceName)."
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
                    name: "Family Archive",
                    volumeName: "photos-nas.local"
                )
            ]
        ),
        imports: ImportExecutionCoordinator(engine: PreviewCatalogEngine())
    )
}

@MainActor
private final class PreviewCatalogEngine: CatalogEngineClient, ImportExecutionEngine {
    func validateCatalog(path: String, intent: String) async throws
        -> EngineSupervisor.CatalogLocation {
        .init(
            path: path,
            volumeName: "photos-nas.local",
            displayName: "Family Archive",
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
            displayName: "Family Archive",
            detail: "Catalog format 2"
        )
    }

    func catalogCollections(
        path: String
    ) async throws -> [EngineSupervisor.CatalogCollection] {
        [
            .dated(
                identity: .dated(
                    year: "2019",
                    directory: "2019-07-14 +6d Brittany"
                ),
                label: "Brittany",
                startDate: "2019-07-14",
                endDate: "2019-07-20"
            ),
            .ongoing(
                identity: .ongoing(key: "kids"),
                label: "Kids",
                parts: [
                    .init(
                        year: 2019,
                        yearDirectory: "2019",
                        collectionDirectory: "z Kids"
                    )
                ]
            )
        ]
    }

    func catalogBrowse(
        path: String,
        selection: EngineSupervisor.CatalogCollectionIdentity?
    ) async throws -> EngineSupervisor.CatalogBrowseSnapshot {
        .init(navigation: [.allPhotos], collections: [])
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
            suggestedAcceptedDate: "2026-10-10",
            suggestedRangeEnd: "2026-10-12",
            pendingItemCount: 4,
            unresolvedItemCount: 0,
            datetimeSources: [
                .init(
                    source: .containerCreationTime,
                    state: .needsConfirmation,
                    itemCount: 4,
                    requiresClock: false,
                    supportsUTCConversion: true,
                    asShownTime: "12:14",
                    convertedFromUTCTime: "14:14"
                )
            ]
        )
    }

    func reviewImport(
        catalogPath: String,
        sourcePath: String,
        destination: EngineSupervisor.ImportDestination,
        dateResolution: EngineSupervisor.ImportDateResolution?
    ) async throws -> EngineSupervisor.ImportReview {
        let scan = try await scanImport(sourcePath: sourcePath)
        let collectionName: String
        let destinationPath: String
        switch destination {
        case .create(let name):
            collectionName = name
            destinationPath = "\(catalogPath)/2026/2026-10-10 \(name)"
        case .existing(.dated(let year, let directory)):
            collectionName = "Brittany"
            destinationPath = "\(catalogPath)/\(year)/\(directory)"
        case .existing(.ongoing):
            collectionName = "Kids"
            destinationPath = "\(catalogPath)/2019/z Kids"
        }
        return .init(
            scan: scan,
            collectionName: collectionName,
            destination: destination,
            destinationPath: destinationPath,
            acceptedDate: "2026-10-10",
            dateResolution: dateResolution,
            requiredDecisionCount: dateResolution == nil ? 1 : 0,
            transferCount: dateResolution == nil ? 8 : scan.mediaCount,
            skippedCount: dateResolution == nil ? 4 : 0,
            leftOnSourceCount: dateResolution == nil ? 6 : scan.unsupportedCount,
            operationCount: (dateResolution == nil ? 8 : scan.mediaCount) + 2,
            preflight: .init(
                representativeFileNames: [
                    "2026-10-10 12-14-00 001.jpg",
                    "2026-10-10 12-14-00 001.xmp",
                    "2026-10-10 14-15-00 001.mov"
                ],
                requiredBytes: scan.totalBytes,
                availableBytes: 500_000_000_000,
                availableSpace: .passed,
                writeProbe: .passed,
                portableNames: .passed,
                destinationConflicts: .passed
            ),
            canStart: true,
            reviewedPlan: "preview-reviewed-plan"
        )
    }

    func startImportExecution(reviewedPlan: String) async throws -> String {
        "preview-operation"
    }
    func inspectUnfinishedOperation(
        catalogPath: String
    ) async throws -> EngineSupervisor.UnfinishedOperation? { nil }
    func recoverOperation(catalogPath: String, operationID: String) async throws -> String {
        "preview-recovery"
    }
    func cancel(
        operationID: String
    ) async throws -> EngineSupervisor.CancellationAccepted {
        .init(operationID: operationID)
    }
    func events(
        for operationID: String
    ) async throws -> AsyncThrowingStream<EngineSupervisor.OperationEvent, any Error> {
        AsyncThrowingStream { $0.finish() }
    }
}
