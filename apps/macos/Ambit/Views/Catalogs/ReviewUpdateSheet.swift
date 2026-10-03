import SwiftUI

/// Shows what a catalog update would install and applies exactly that plan.
struct ReviewUpdateSheet: View {
    let review: CatalogUpdateReview
    let model: CatalogUpdatesModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Review Catalog Update")
                .font(.title2.bold())
                .accessibilityAddTraits(.isHeader)

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    revisions
                    content
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }

            Divider()
            buttons
        }
        .padding(20)
        .frame(minWidth: 520, idealWidth: 600, minHeight: 360, idealHeight: 480)
        .interactiveDismissDisabled(review.isRunning && !review.canCancel)
    }

    private var revisions: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Revisions")
                .font(.headline)
            ForEach(review.revisions, id: \.catalog) { revision in
                let installed = model.records[revision.catalog]?.installed
                HStack {
                    Text(revision.catalog)
                        .font(.body.weight(.medium))
                    Text("\(abbreviatedCommit(installed) ?? "?") → \(abbreviatedCommit(revision.commit) ?? "")")
                        .font(.body.monospaced())
                        .foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .combine)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch review.phase {
        case .reviewing, .applying:
            HStack(spacing: 8) {
                ProgressView()
                    .controlSize(.small)
                Text(progressText)
                    .foregroundStyle(.secondary)
            }
        case let .ready(summary, refreshed):
            if refreshed {
                Label(
                    "The setup changed after the last review, so the plan was refreshed. Review it again before updating.",
                    systemImage: "arrow.clockwise.circle"
                )
                .foregroundStyle(.orange)
                .accessibilityIdentifier("updateReview.refreshed")
            }
            UpdateSummaryView(summary: summary)
        case let .reviewFailed(error):
            ErrorBlock(title: String(localized: "The update could not be reviewed."), error: error)
        case let .finished(outcome):
            OutcomeView(outcome: outcome)
        }
    }

    private var progressText: String {
        guard let progress = review.operation?.progress else {
            return review.phase == .applying
                ? String(localized: "Updating…") : String(localized: "Planning the update…")
        }

        let stage = progress.stage.updateTitle
        guard progress.total > 0 else {
            return stage
        }
        return String(localized: "\(stage) (\(progress.current) of \(progress.total))")
    }

    @ViewBuilder
    private var buttons: some View {
        HStack {
            switch review.phase {
            case .reviewing, .applying:
                Spacer()
                Button("Cancel", role: .cancel) {
                    model.closeReview()
                }
                .disabled(!review.canCancel)
                .accessibilityIdentifier("updateReview.cancel")
            case let .ready(summary, _):
                Button("Cancel", role: .cancel) {
                    model.closeReview()
                }
                .keyboardShortcut(.cancelAction)
                .accessibilityIdentifier("updateReview.cancel")
                Spacer()
                Button("Refresh Plan") {
                    Task { await review.reviewAgain() }
                }
                Button("Update") {
                    Task { await review.apply() }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!summary.canApply)
                .accessibilityIdentifier("updateReview.apply")
            case .reviewFailed, .finished:
                if !review.didInstall {
                    Button("Review Again") {
                        Task { await review.reviewAgain() }
                    }
                    .accessibilityIdentifier("updateReview.again")
                }
                Spacer()
                Button("Done") {
                    model.closeReview()
                }
                .keyboardShortcut(.defaultAction)
                .accessibilityIdentifier("updateReview.done")
            }
        }
    }
}

private struct UpdateSummaryView: View {
    let summary: ReviewSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if !summary.blockers.isEmpty {
                section(String(localized: "Problems that prevent updating")) {
                    ForEach(Array(summary.blockers.enumerated()), id: \.offset) { _, blocker in
                        Label(blocker.message, systemImage: "xmark.octagon")
                            .foregroundStyle(.red)
                    }
                }
            }

            section(String(localized: "Capabilities")) {
                if summary.diff.added.isEmpty, summary.diff.removed.isEmpty {
                    Text("No capabilities are added or removed.")
                        .foregroundStyle(.secondary)
                }
                ForEach(summary.diff.added, id: \.self) { item in
                    Label("\(item.name) (\(item.kind.rawValue), \(item.catalog))", systemImage: "plus.circle")
                        .foregroundStyle(.green)
                }
                ForEach(summary.diff.removed, id: \.self) { item in
                    Label("\(item.name) (\(item.kind.rawValue), \(item.catalog))", systemImage: "minus.circle")
                        .foregroundStyle(.red)
                }
            }
            .accessibilityIdentifier("updateReview.capabilities")

            if !summary.writes.isEmpty || !summary.removals.isEmpty {
                section(String(localized: "Files")) {
                    ForEach(summary.writes, id: \.self) { write in
                        Label(write.path, systemImage: "square.and.pencil")
                            .font(.callout.monospaced())
                    }
                    ForEach(summary.removals, id: \.self) { removal in
                        Label(removal.path, systemImage: "trash")
                            .font(.callout.monospaced())
                    }
                }
            }

            if !summary.skipped.isEmpty || !summary.limitations.isEmpty {
                section(String(localized: "Agent tool limitations")) {
                    ForEach(summary.skipped, id: \.self) { skipped in
                        Text("\(skipped.hook.name) is skipped for \(skipped.harness): \(skipped.reason)")
                    }
                    ForEach(summary.limitations, id: \.self) { finding in
                        Text(finding.message)
                    }
                }
            }
        }
    }

    private func section(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.headline)
            content()
        }
    }
}

private struct OutcomeView: View {
    let outcome: ReviewModel.Outcome

    var body: some View {
        switch outcome {
        case .installed:
            Label("Catalogs updated.", systemImage: "checkmark.circle.fill")
                .foregroundStyle(.green)
                .accessibilityIdentifier("updateReview.installed")
        case let .notFullyInstalled(_, error):
            ErrorBlock(title: String(localized: "Changes not fully installed"), error: error)
        case let .saveFailed(error):
            ErrorBlock(title: String(localized: "The update was not applied. Nothing changed."), error: error)
        case let .stale(error):
            ErrorBlock(title: String(localized: "The setup changed after the review."), error: error)
        case .canceled:
            Text("The update was canceled. Nothing changed.")
                .foregroundStyle(.secondary)
        }
    }
}

private struct ErrorBlock: View {
    let title: String
    let error: EngineError

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(title, systemImage: "exclamationmark.triangle.fill")
                .font(.headline)
                .foregroundStyle(.orange)
            Text(error.message)
            ForEach(Array(error.detail.enumerated()), id: \.offset) { _, line in
                Text(line)
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("updateReview.error")
    }
}

extension Blocker {
    fileprivate var message: String {
        switch self {
        case let .config(error), let .resolution(error): error.message
        case let .unmatched(_, error): error.message
        case let .ownership(conflict): "\(conflict.path): \(conflict.message)"
        }
    }
}

extension Stage {
    fileprivate var updateTitle: String {
        switch self {
        case .loadingCatalogs: String(localized: "Loading catalogs")
        case .fetching: String(localized: "Fetching")
        case .resolving: String(localized: "Resolving")
        case .planning: String(localized: "Planning")
        case .checkingOwnership: String(localized: "Checking files")
        case .savingConfig: String(localized: "Saving the configuration")
        case .writingFiles: String(localized: "Writing files")
        case .removingFiles: String(localized: "Removing files")
        case .writingRecords: String(localized: "Recording the installation")
        }
    }
}
