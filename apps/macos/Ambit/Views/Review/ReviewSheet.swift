import SwiftUI

/// The review of a setup's draft, then its apply progress and outcome.
struct ReviewSheet: View {
    let setup: SetupModel
    let review: ReviewModel

    var body: some View {
        VStack(spacing: 0) {
            Group {
                switch review.phase {
                case .reviewing:
                    ApplyProgressView(
                        title: "Reviewing changes…", operation: review.operation, canCancel: review.canCancel,
                        cancel: setup.closeReview)
                case let .ready(summary):
                    ReviewSummaryView(setup: setup, summary: summary)
                case let .reviewFailed(error):
                    ReviewFailedView(error: error)
                case .applying:
                    ApplyProgressView(
                        title: review.mode == .apply ? "Applying changes…" : "Retrying installation…",
                        operation: review.operation, canCancel: review.canCancel, cancel: review.cancel)
                case let .finished(outcome):
                    ApplyResultView(outcome: outcome, mode: review.mode)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)

            Divider()
            buttons
                .padding(16)
        }
        .frame(minWidth: 560, idealWidth: 640, minHeight: 360, idealHeight: 520)
        .interactiveDismissDisabled(review.isRunning)
    }

    @ViewBuilder
    private var buttons: some View {
        HStack {
            switch review.phase {
            case .reviewing, .applying:
                Spacer()
            case let .ready(summary):
                Button("Cancel", role: .cancel) {
                    setup.closeReview()
                }
                .keyboardShortcut(.cancelAction)
                .accessibilityIdentifier("review.cancel")
                Spacer()
                Button("Apply") {
                    Task { await review.apply() }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(!summary.canApply)
                .help(summary.canApply ? "Save the configuration and install it" : "Fix the problems listed first")
                .accessibilityIdentifier("review.apply")
            case .reviewFailed:
                Button("Close", role: .cancel) {
                    setup.closeReview()
                }
                .keyboardShortcut(.cancelAction)
                .accessibilityIdentifier("review.close")
                Spacer()
                Button("Review Again") {
                    Task { await reviewAgain() }
                }
                .buttonStyle(.borderedProminent)
                .accessibilityIdentifier("review.again")
            case let .finished(outcome):
                finishedButtons(outcome)
            }
        }
    }

    @ViewBuilder
    private func finishedButtons(_ outcome: ReviewModel.Outcome) -> some View {
        switch outcome {
        case .installed:
            Spacer()
            Button("Done") {
                setup.closeReview()
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .accessibilityIdentifier("review.done")
        case .notFullyInstalled:
            Button("Close", role: .cancel) {
                setup.closeReview()
            }
            .keyboardShortcut(.cancelAction)
            .accessibilityIdentifier("review.close")
            Spacer()
            Button("Retry Installation") {
                Task { await review.retryInstall() }
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .accessibilityIdentifier("review.retry")
        case .saveFailed:
            Spacer()
            Button("Close") {
                setup.closeReview()
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .accessibilityIdentifier("review.close")
        case .stale, .canceled:
            Button("Close", role: .cancel) {
                setup.closeReview()
            }
            .keyboardShortcut(.cancelAction)
            .accessibilityIdentifier("review.close")
            Spacer()
            if review.mode == .apply {
                Button("Review Again") {
                    Task { await reviewAgain() }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .accessibilityIdentifier("review.again")
            }
        }
    }

    /// When the file changed on disk, closes the sheet so the setup can ask about it.
    private func reviewAgain() async {
        if !(await review.reviewAgain()) {
            setup.closeReview()
        }
    }
}

private struct ReviewFailedView: View {
    let error: EngineError

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("The changes could not be reviewed", systemImage: "xmark.octagon.fill")
                .font(.title3.bold())
                .foregroundStyle(.red)
            EngineErrorText(error: error)
            Text("Nothing was written. Your changes are still in the draft.")
                .foregroundStyle(.secondary)
        }
        .padding(20)
        .accessibilityIdentifier("review.failed")
    }
}

/// What Apply would do. Empty sections are omitted, except configuration and managed files,
/// where "nothing changes" is itself worth stating.
struct ReviewSummaryView: View {
    let setup: SetupModel
    let summary: ReviewSummary

    var body: some View {
        List {
            Section {
                Text(
                    setup.draft?.isNew == true
                        ? "Applying creates \(setup.fileName) in \(setup.root.path) and installs it."
                        : "Applying saves \(setup.fileName) and installs the result."
                )
                .foregroundStyle(.secondary)
            }

            if !summary.blockers.isEmpty {
                Section("Problems to fix before applying") {
                    ForEach(Array(summary.blockers.enumerated()), id: \.offset) { _, blocker in
                        BlockerRow(blocker: blocker)
                    }
                }
                .accessibilityIdentifier("review.blockers")
            }

            configSection

            if !summary.diff.added.isEmpty || !summary.diff.removed.isEmpty || summary.lockChanged {
                Section("Capabilities") {
                    ForEach(summary.diff.added, id: \.self) { item in
                        ChangeRow(symbol: "plus.circle.fill", tint: .green, text: item.reviewTitle, detail: "Installed")
                    }
                    ForEach(summary.diff.removed, id: \.self) { item in
                        ChangeRow(symbol: "minus.circle.fill", tint: .red, text: item.reviewTitle, detail: "Removed")
                    }
                    if summary.lockChanged {
                        ChangeRow(
                            symbol: "lock.rotation", tint: .secondary,
                            text: String(localized: "The recorded catalog revisions change."), detail: nil)
                    }
                }
            }

            Section("Managed files") {
                if summary.writes.isEmpty && summary.removals.isEmpty {
                    Text("No installed files change.")
                        .foregroundStyle(.secondary)
                }
                ForEach(summary.writes, id: \.self) { write in
                    PathRow(symbol: "square.and.pencil", path: write.path, kind: write.kind)
                }
                ForEach(summary.removals, id: \.self) { removal in
                    PathRow(symbol: "trash", path: removal.path, kind: removal.kind)
                }
            }

            if !summary.skipped.isEmpty || !summary.limitations.isEmpty {
                Section("Not installed for some agent tools") {
                    ForEach(summary.skipped, id: \.self) { skipped in
                        ChangeRow(
                            symbol: "slash.circle", tint: .orange,
                            text: "\(skipped.hook.reviewTitle) · \(toolName(skipped.harness))", detail: skipped.reason)
                    }
                    ForEach(summary.limitations, id: \.self) { finding in
                        ChangeRow(
                            symbol: "info.circle", tint: .secondary, text: finding.message,
                            detail: finding.detail.joined(separator: "\n"))
                    }
                }
            }
        }
        .listStyle(.inset)
        .accessibilityIdentifier("review.summary")
    }

    @ViewBuilder
    private var configSection: some View {
        let config = summary.config
        let rows = configRows(config)

        Section("Configuration") {
            if rows.isEmpty {
                Text("The configuration doesn't change.")
                    .foregroundStyle(.secondary)
            }
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                ChangeRow(symbol: row.symbol, tint: row.tint, text: row.text, detail: row.detail)
            }
        }
    }

    private struct Row {
        var symbol: String
        var tint: Color
        var text: String
        var detail: String?
    }

    private func configRows(_ config: ConfigChanges) -> [Row] {
        var rows: [Row] = []
        rows += config.harnessesAdded.map {
            Row(symbol: "plus.circle", tint: .green, text: String(localized: "Agent tool \(toolName($0))"), detail: nil)
        }
        rows += config.harnessesRemoved.map {
            Row(symbol: "minus.circle", tint: .red, text: String(localized: "Agent tool \(toolName($0))"), detail: nil)
        }
        rows += config.catalogsAdded.map {
            Row(symbol: "plus.circle", tint: .green, text: String(localized: "Catalog \($0.name)"), detail: $0.sourceDescription)
        }
        rows += config.catalogsRemoved.map {
            Row(symbol: "minus.circle", tint: .red, text: String(localized: "Catalog \($0.name)"), detail: $0.sourceDescription)
        }
        rows += config.catalogsChanged.map {
            Row(
                symbol: "arrow.triangle.2.circlepath", tint: .blue, text: String(localized: "Catalog \($0.new.name)"),
                detail: "\($0.old.sourceDescription) → \($0.new.sourceDescription)")
        }
        rows += config.catalogsRenamed.map {
            Row(
                symbol: "pencil.circle", tint: .blue, text: String(localized: "Catalog \($0.from) renamed to \($0.to)"),
                detail: nil)
        }
        rows += config.entriesAdded.map {
            Row(symbol: "plus.circle", tint: .green, text: $0.reviewTitle, detail: $0.reviewDetail)
        }
        rows += config.entriesRemoved.map {
            Row(symbol: "minus.circle", tint: .red, text: $0.reviewTitle, detail: $0.reviewDetail)
        }
        return rows
    }

    private func toolName(_ harness: String) -> String {
        setup.engine.supportedAgentTools().first { $0.id == harness }?.displayName ?? harness
    }
}

/// A problem that blocks Apply, with what the user can do about it. The app never offers to
/// take over or overwrite a file it does not manage.
private struct BlockerRow: View {
    let blocker: Blocker

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "xmark.octagon.fill")
                .foregroundStyle(.red)
            VStack(alignment: .leading, spacing: 4) {
                content
            }
        }
        .accessibilityElement(children: .combine)
    }

    @ViewBuilder
    private var content: some View {
        switch blocker {
        case let .config(error), let .resolution(error):
            EngineErrorText(error: error)
            Text("Correct the configuration in the setup's views, then review again.")
                .font(.callout)
                .foregroundStyle(.secondary)
        case let .unmatched(entry, error):
            Text("\(entry.reviewTitle) matches nothing")
                .fontWeight(.medium)
            EngineErrorText(error: error)
            Text("Edit or remove this entry in Capabilities, then review again.")
                .font(.callout)
                .foregroundStyle(.secondary)
        case let .ownership(conflict):
            Text(conflict.path)
                .fontWeight(.medium)
                .textSelection(.enabled)
            if let key = conflict.key {
                Text("Entry \(key)")
                    .font(.callout.monospaced())
            }
            Text(conflict.message)
            ForEach(Array(conflict.detail.enumerated()), id: \.offset) { _, line in
                Text(line)
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Text(
                "Ambit didn't create this, so it won't change it. Move or delete it yourself, or deselect the capability that installs there, then review again."
            )
            .font(.callout)
            .foregroundStyle(.secondary)
        }
    }
}

private struct ChangeRow: View {
    let symbol: String
    let tint: Color
    let text: String
    let detail: String?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: symbol)
                .foregroundStyle(tint)
            VStack(alignment: .leading, spacing: 2) {
                Text(text)
                if let detail, !detail.isEmpty {
                    Text(detail)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
}

private struct PathRow: View {
    let symbol: String
    let path: String
    let kind: String

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: symbol)
                .foregroundStyle(.secondary)
            Text(path)
                .font(.callout.monospaced())
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
                .help(path)
            Spacer()
            Text(kind)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .accessibilityElement(children: .combine)
    }
}

extension ItemRef {
    /// "skill team/review".
    var reviewTitle: String { "\(kind.rawValue) \(catalog)/\(name)" }
}

extension SelectionEntry {
    var reviewTitle: String {
        let target = catalog.map { "\($0)/\(pattern)" } ?? pattern
        return "\(kind.rawValue) \(target)"
    }

    var reviewDetail: String {
        isRule ? String(localized: "Selection rule") : String(localized: "Selection")
    }
}

extension CatalogEntry {
    var sourceDescription: String {
        gitRef.map { "\(source) @ \($0)" } ?? source
    }
}
