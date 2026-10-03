import SwiftUI

/// Check for catalog updates and Review update for one setup. Self-contained, so the Catalogs
/// view embeds it with `CatalogUpdatesView(setup: setup)`.
struct CatalogUpdatesView: View {
    let setup: SetupModel
    @Environment(AppModel.self) private var app

    var body: some View {
        CatalogUpdatesContent(setup: setup, model: setup.catalogUpdates(store: app.environment.stateStore))
    }
}

private struct CatalogUpdatesContent: View {
    let setup: SetupModel
    @Bindable var model: CatalogUpdatesModel

    var body: some View {
        GroupBox {
            VStack(alignment: .leading, spacing: 12) {
                header

                if model.isBlockedByDraft, !model.availableUpdates.isEmpty {
                    DraftBlocksUpdateNotice(setup: setup)
                }

                if model.rows.isEmpty {
                    Text("This setup has no saved catalogs to update.")
                        .foregroundStyle(.secondary)
                } else {
                    VStack(spacing: 0) {
                        ForEach(model.rows) { row in
                            if row.id != model.rows.first?.id {
                                Divider()
                            }
                            CatalogUpdateRow(row: row, model: model)
                        }
                    }
                }

                if model.availableUpdates.count > 1 {
                    HStack {
                        Spacer()
                        Button("Update All…") {
                            Task { await model.reviewUpdate() }
                        }
                        .disabled(model.isBlockedByDraft || model.isChecking)
                        .accessibilityIdentifier("catalogUpdates.updateAll")
                    }
                }
            }
            .padding(8)
        } label: {
            Label("Catalog Updates", systemImage: "arrow.triangle.2.circlepath")
                .font(.headline)
        }
        .sheet(isPresented: reviewPresented) {
            if let review = model.review {
                ReviewUpdateSheet(review: review, model: model)
            }
        }
        .alert(item: $model.presentedError) { error in
            Alert(title: Text(error.title), message: Text(error.message))
        }
    }

    private var reviewPresented: Binding<Bool> {
        Binding(
            get: { model.review != nil },
            set: { presented in
                if !presented {
                    model.closeReview()
                }
            })
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Ambit checks for newer catalog revisions only when you ask.")
                    .font(.callout)
                lastChecked
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("catalogUpdates.lastChecked")
            }

            Spacer()

            if model.isChecking {
                CheckProgress(model: model)
            } else {
                Button("Check for Updates") {
                    Task { await model.checkAll() }
                }
                .disabled(model.checkableCatalogs.isEmpty || setup.operations.isBusy)
                .help("Check every remote catalog that is not pinned to a commit")
                .accessibilityIdentifier("catalogUpdates.check")
            }
        }
    }

    @ViewBuilder
    private var lastChecked: some View {
        if let date = model.lastCheckedAt {
            Text("Last checked \(date, format: .relative(presentation: .named))")
        } else {
            Text("Never checked")
        }
    }
}

private struct CheckProgress: View {
    let model: CatalogUpdatesModel

    var body: some View {
        HStack(spacing: 8) {
            ProgressView()
                .controlSize(.small)
            if let subject = model.operation?.progress?.subject, !subject.isEmpty {
                Text("Checking \(subject)…")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            } else {
                Text("Checking…")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Button("Cancel") {
                model.cancelCheck()
            }
            .accessibilityIdentifier("catalogUpdates.cancel")
        }
    }
}

/// Updates work on the saved config, so a draft must be applied or discarded first.
private struct DraftBlocksUpdateNotice: View {
    let setup: SetupModel

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Label(
                "Apply or discard your pending changes before updating catalogs.",
                systemImage: "exclamationmark.triangle"
            )
            .foregroundStyle(.orange)
            Spacer()
            Button("Discard Changes", role: .destructive) {
                setup.discardChanges()
            }
            .accessibilityIdentifier("catalogUpdates.discardDraft")
            Button("Review Changes…") {
                Task { await setup.startReview() }
            }
            .accessibilityIdentifier("catalogUpdates.reviewDraft")
        }
        .font(.callout)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("catalogUpdates.draftNotice")
    }
}

private struct CatalogUpdateRow: View {
    let row: CatalogUpdatesModel.Row
    let model: CatalogUpdatesModel

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(row.catalog.name)
                    .font(.body.weight(.medium))
                Text(row.catalog.source)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .frame(minWidth: 160, alignment: .leading)

            CatalogUpdateStatusView(status: row.status)
                .accessibilityIdentifier("catalogUpdates.status.\(row.id)")

            Spacer()

            action
        }
        .padding(.vertical, 8)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("catalogUpdates.row.\(row.id)")
    }

    @ViewBuilder
    private var action: some View {
        switch row.status {
        case .updateAvailable:
            Button("Review Update…") {
                Task { await model.reviewUpdate([row.id]) }
            }
            .disabled(model.isBlockedByDraft || model.isChecking)
            .accessibilityIdentifier("catalogUpdates.update.\(row.id)")
        case .failed:
            Button("Retry") {
                Task { await model.check([row.id]) }
            }
            .disabled(model.isChecking)
            .accessibilityIdentifier("catalogUpdates.retry.\(row.id)")
        case .unchecked, .upToDate:
            Button("Check") {
                Task { await model.check([row.id]) }
            }
            .disabled(model.isChecking)
            .accessibilityIdentifier("catalogUpdates.checkOne.\(row.id)")
        case .checking, .pinned, .localFiles:
            EmptyView()
        }
    }
}

private struct CatalogUpdateStatusView: View {
    let status: CatalogUpdatesModel.Status

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Label(title, systemImage: symbol)
                .foregroundStyle(tint)
            if let detail {
                Text(detail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(detail.map { "\(title). \($0)" } ?? title)
    }

    private var title: String {
        switch status {
        case .unchecked: String(localized: "Not checked")
        case .checking: String(localized: "Checking…")
        case .upToDate: String(localized: "Up to date")
        case .updateAvailable: String(localized: "Update available")
        case .pinned: String(localized: "Pinned")
        case .localFiles: String(localized: "Uses local files")
        case .failed: String(localized: "Check failed")
        }
    }

    private var detail: String? {
        switch status {
        case .unchecked, .checking:
            return nil
        case let .upToDate(checkedAt):
            return String(localized: "Checked \(checkedAt.formatted(.relative(presentation: .named)))")
        case let .updateAvailable(record):
            let revision = String(
                localized: "\(abbreviatedCommit(record.installed) ?? "?") → \(abbreviatedCommit(record.latest) ?? "?")")
            let changes = changeSummary(added: record.added.count, removed: record.removed.count)
            return [revision, changes].compactMap(\.self).joined(separator: " · ")
        case let .pinned(commit):
            return abbreviatedCommit(commit).map { String(localized: "Pinned to commit \($0)") }
        case .localFiles:
            return String(localized: "Changes in the folder apply when you install")
        case let .failed(message, lastKnown, _):
            if let installed = abbreviatedCommit(lastKnown) {
                return String(localized: "\(message) Installed revision: \(installed).")
            }
            return message
        }
    }

    private func changeSummary(added: Int, removed: Int) -> String? {
        switch (added, removed) {
        case (0, 0): String(localized: "No capability changes")
        case (_, 0): String(localized: "\(added) capabilities added")
        case (0, _): String(localized: "\(removed) capabilities removed")
        default: String(localized: "\(added) added, \(removed) removed")
        }
    }

    private var symbol: String {
        switch status {
        case .unchecked: "questionmark.circle"
        case .checking: "arrow.triangle.2.circlepath"
        case .upToDate: "checkmark.circle"
        case .updateAvailable: "arrow.up.circle"
        case .pinned: "pin"
        case .localFiles: "folder"
        case .failed: "exclamationmark.triangle"
        }
    }

    private var tint: Color {
        switch status {
        case .unchecked, .checking, .pinned, .localFiles: .secondary
        case .upToDate: .green
        case .updateAvailable: .blue
        case .failed: .orange
        }
    }
}

/// The first seven characters of a commit id.
func abbreviatedCommit(_ commit: String?) -> String? {
    commit.map { String($0.prefix(7)) }
}
