import SwiftUI

/// The setup's catalogs: where each comes from, which revision it follows, and whether its
/// content is available. Add, edit and remove stage changes in the draft.
struct CatalogsView: View {
    let setup: SetupModel
    @Environment(AppModel.self) private var app

    private var model: CatalogsModel { setup.catalogs }

    var body: some View {
        @Bindable var model = model

        VStack(alignment: .leading, spacing: 12) {
            header

            if let error = model.readError {
                CatalogProblemView(
                    title: String(localized: "Catalogs could not be read"), error: error, account: app.account,
                    retry: { Task { await model.refresh() } })
            }

            if !model.unmatched.isEmpty {
                UnmatchedEntriesView(entries: model.unmatched, remove: model.removeUnmatched)
            }

            if setup.configSummary == nil {
                ContentUnavailableView(
                    "No Configuration", systemImage: "books.vertical",
                    description: Text("Set up this folder before adding catalogs."))
            } else if model.rows.isEmpty {
                ContentUnavailableView {
                    Label("No Catalogs", systemImage: "books.vertical")
                } description: {
                    Text("Add a local folder or a Git repository to take capabilities from.")
                } actions: {
                    Button("Add Catalog…") { model.startAdding() }
                }
            } else {
                List {
                    ForEach(model.rows) { row in
                        CatalogRowView(setup: setup, row: row, account: app.account)
                    }
                }
                .accessibilityIdentifier("catalogs.list")
            }

            // Catalog update checks (CatalogUpdatesView) are their own area and attach here.
        }
        .padding(.top, 8)
        .task(id: setup.configSummary) {
            await model.refresh()
        }
        .sheet(item: $model.editor) { editor in
            CatalogEditorSheet(editor: editor, catalogs: model, folderPicker: app.environment.folderPicker)
        }
        .sheet(item: $model.removal) { removal in
            RemoveCatalogSheet(removal: removal, catalogs: model)
        }
        .alert(item: $model.presentedError) { error in
            Alert(title: Text(error.title), message: Text(error.message))
        }
    }

    private var header: some View {
        HStack(spacing: 12) {
            Text("Catalogs are the folders and Git repositories this setup takes capabilities from.")
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)

            if model.isFetching {
                CatalogFetchProgressView(operation: setup.operations.current) { model.cancelLoad() }
            } else if model.canLoad {
                Button("Load Catalogs", systemImage: "arrow.down.circle") { model.loadMissing() }
                    .help("Download the catalogs that are not available on this Mac, at their configured revisions")
                    .accessibilityIdentifier("catalogs.load")
            }

            Button("Add Catalog…", systemImage: "plus") { model.startAdding() }
                .disabled(setup.configSummary == nil)
                .accessibilityIdentifier("catalogs.add")
        }
    }
}

/// The running operation's stage, with Cancel while it can still be canceled.
struct CatalogFetchProgressView: View {
    let operation: OperationRunner.Operation?
    let cancel: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text(label)
                .font(.callout)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Button("Cancel", action: cancel)
                .disabled(operation?.isCancellable == false)
        }
    }

    private var label: String {
        guard let progress = operation?.progress else {
            return operation?.title ?? String(localized: "Waiting…")
        }
        let stage =
            switch progress.stage {
            case .fetching: String(localized: "Downloading")
            case .loadingCatalogs: String(localized: "Reading")
            default: String(localized: "Working on")
            }
        return progress.subject.isEmpty ? stage : "\(stage) \(progress.subject)"
    }
}

private struct CatalogRowView: View {
    let setup: SetupModel
    let row: CatalogsModel.Row
    let account: GitHubAccountModel

    private var model: CatalogsModel { setup.catalogs }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(row.entry.name)
                    .font(.headline)
                CatalogKindLabel(kind: row.entry.sourceKind)
                if let change = row.change {
                    Text(change == .added ? "Added" : "Changed")
                        .font(.caption.weight(.medium))
                        .foregroundStyle(.orange)
                }
                Spacer()
                Button("Edit…") { model.startEditing(row.entry.name) }
                    .accessibilityIdentifier("catalog.edit.\(row.entry.name)")
                Button("Remove…", role: .destructive) { model.startRemoving(row.entry.name) }
                    .accessibilityIdentifier("catalog.remove.\(row.entry.name)")
            }

            Text(row.entry.source)
                .font(.callout.monospaced())
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
            if case .local = row.entry.sourceKind, setup.resolvedLocalPath(row.entry.source) != row.entry.source {
                Text(setup.resolvedLocalPath(row.entry.source))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }

            HStack(spacing: 16) {
                if let revision = CatalogText.revision(row.entry) {
                    Label(revision, systemImage: "arrow.triangle.branch")
                }
                availability
            }
            .font(.callout)

            if case let .failed(error) = row.availability {
                CatalogProblemView(
                    title: String(localized: "\(row.entry.name) could not be loaded"), error: error, account: account,
                    retry: { model.loadMissing() })
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("catalog.row.\(row.entry.name)")
        .contextMenu {
            Button("Edit…") { model.startEditing(row.entry.name) }
            Button("Remove…", role: .destructive) { model.startRemoving(row.entry.name) }
        }
    }

    @ViewBuilder private var availability: some View {
        switch row.availability {
        case nil:
            Label("Checking…", systemImage: "clock")
                .foregroundStyle(.secondary)
        case let .available(_, local) where local:
            Label("Uses local files", systemImage: "folder")
                .foregroundStyle(.secondary)
        case let .available(commit, _):
            Label(
                commit.map { String(localized: "Available offline at \(CatalogText.short($0))") }
                    ?? String(localized: "Available offline"),
                systemImage: "checkmark.circle"
            )
            .foregroundStyle(.secondary)
            .help("Content cached on this Mac. This does not check the remote for newer revisions.")
        case .notCached:
            HStack(spacing: 8) {
                Label("Not downloaded", systemImage: "icloud.and.arrow.down")
                    .foregroundStyle(.orange)
                if !model.isFetching {
                    Button("Load Catalogs") { model.loadMissing() }
                        .controlSize(.small)
                }
            }
        case .failed:
            Label("Could not load", systemImage: "exclamationmark.triangle")
                .foregroundStyle(.red)
        }
    }
}

struct CatalogKindLabel: View {
    let kind: SourceKind?

    var body: some View {
        Label(CatalogText.kind(kind), systemImage: symbol)
            .font(.caption.weight(.medium))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(.quaternary, in: Capsule())
    }

    private var symbol: String {
        switch kind {
        case .local: "folder"
        case .git(_, true, _): "chevron.left.forwardslash.chevron.right"
        case .git: "arrow.triangle.branch"
        case nil: "questionmark"
        }
    }
}

enum CatalogText {
    static func kind(_ kind: SourceKind?) -> String {
        switch kind {
        case .local: String(localized: "Local folder")
        case .git(_, true, _): String(localized: "GitHub")
        case .git: String(localized: "Git")
        case nil: String(localized: "Unknown source")
        }
    }

    /// The revision a remote catalog follows. `nil` for local folders.
    static func revision(_ entry: CatalogEntry) -> String? {
        switch entry.sourceKind {
        case .local, nil:
            nil
        case let .git(_, _, commitRef):
            if let ref = entry.gitRef {
                commitRef ? String(localized: "Pinned to commit \(short(ref))") : ref
            } else {
                String(localized: "Default branch")
            }
        }
    }

    static func short(_ commit: String) -> String {
        String(commit.prefix(7))
    }
}

/// A load failure with what to do about it. GitHub access problems explain the cause and offer
/// Sign In Again and, for SSO organizations, the authorization page.
struct CatalogProblemView: View {
    let title: String
    let error: EngineError
    let account: GitHubAccountModel
    let retry: () -> Void
    @State private var signingIn = false
    @Environment(\.openURL) private var openURL

    private var explanation: GitHubAccessExplanation? {
        guard case let .network(_, _, kind) = error else {
            return nil
        }
        return GitHubAccessExplanation(
            kind: kind, signedInAs: account.username, authorizationURL: account.authorizationSettingsURL)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(explanation?.title ?? title, systemImage: "exclamationmark.triangle.fill")
                .font(.callout.weight(.semibold))
                .foregroundStyle(.red)
            Text(explanation?.message ?? error.message)
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
            if explanation != nil {
                Text(error.message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            ForEach(Array(error.detail.prefix(4).enumerated()), id: \.offset) { _, line in
                Text(line)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
            HStack {
                Button("Retry", action: retry)
                if let explanation, explanation.offersSignIn, account.isAvailable {
                    Button(account.username == nil ? "Sign In with GitHub…" : "Sign In Again…") { signingIn = true }
                }
                if let url = explanation?.authorizationURL {
                    Button("Authorize on GitHub") { openURL(url) }
                }
            }
            .controlSize(.small)
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.red.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .contain)
        .gitHubSignInSheet(isPresented: $signingIn, account: account)
    }
}

/// Entries that select nothing in their catalog any more. Apply stays blocked until each is
/// fixed or removed.
struct UnmatchedEntriesView: View {
    let entries: [UnmatchedEntry]
    /// Offers Remove Selection per entry when set.
    let remove: ((SelectionEntry) -> Void)?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(
                "^[\(entries.count) selection](inflect: true) no longer resolve. Apply is blocked until they are fixed or removed.",
                systemImage: "exclamationmark.octagon.fill"
            )
            .font(.callout.weight(.semibold))
            .foregroundStyle(.orange)
            ForEach(entries, id: \.entry) { item in
                HStack(alignment: .firstTextBaseline) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(CatalogSelectionText.describe(item.entry))
                            .font(.callout.monospaced())
                        Text(item.error.message)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    if let remove {
                        Button("Remove Selection") { remove(item.entry) }
                            .controlSize(.small)
                    }
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 8))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("catalogs.unmatched")
    }
}

enum CatalogSelectionText {
    static func kind(_ kind: ItemKind) -> String {
        switch kind {
        case .skill: String(localized: "Skill")
        case .pack: String(localized: "Pack")
        case .mcp: String(localized: "MCP server")
        case .hook: String(localized: "Hook")
        }
    }

    static func describe(_ entry: SelectionEntry) -> String {
        let address = entry.catalog.map { "\($0)/\(entry.pattern)" } ?? entry.pattern
        return "\(kind(entry.kind)) \(address)"
    }
}
