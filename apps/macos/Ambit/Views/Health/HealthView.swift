import SwiftUI

struct HealthView: View {
    let setup: SetupModel

    var body: some View {
        let model = setup.health

        Form {
            Section {
                HealthActions(model: model)
                EnvironmentNote()
                if let error = model.error {
                    Label(error.message, systemImage: "xmark.octagon")
                        .foregroundStyle(.red)
                        .accessibilityIdentifier("health.error")
                }
            }

            if !model.hasSavedConfig {
                Section {
                    Label(
                        "This setup has no saved configuration, so nothing is installed yet.",
                        systemImage: "circle.dashed"
                    )
                    .accessibilityIdentifier("health.unconfigured")
                }
            } else {
                if let summary = model.summary {
                    Section {
                        SummaryRow(summary: summary, checkedAt: model.healthCheckedAt, readAt: model.statusReadAt)
                    }
                }

                Section("Capabilities") {
                    if model.status == nil {
                        Text("Status not read yet.").foregroundStyle(.secondary)
                    } else if model.items.isEmpty {
                        Text("This setup installs no capabilities.").foregroundStyle(.secondary)
                    }
                    ForEach(model.items) { item in
                        ItemRow(item: item, model: model)
                    }
                }

                if model.report != nil {
                    Section("Findings") {
                        if model.issues.isEmpty {
                            Label("No problems found.", systemImage: "checkmark.circle")
                                .foregroundStyle(.green)
                                .accessibilityIdentifier("health.noFindings")
                        }
                        ForEach(model.issues) { issue in
                            IssueView(issue: issue, model: model)
                        }
                    }
                }

                if !model.toolLimitations.isEmpty {
                    Section("Agent tool limitations") {
                        ForEach(model.toolLimitations) { tool in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(tool.displayName).font(.headline)
                                ForEach(tool.limitations, id: \.self) { limitation in
                                    Label(limitation, systemImage: "exclamationmark.triangle")
                                        .fixedSize(horizontal: false, vertical: true)
                                }
                            }
                        }
                    }
                }
            }
        }
        .formStyle(.grouped)
        .task(id: setup.id) {
            await model.refreshStatus()
        }
        .onChange(of: setup.review == nil) { _, closed in
            // A review that applied changes the installation; read it again once the sheet closes.
            if closed {
                Task { await model.refreshStatus() }
            }
        }
    }
}

private struct HealthActions: View {
    let model: HealthModel

    var body: some View {
        HStack {
            Button {
                Task { await model.refreshStatus() }
            } label: {
                Label("Refresh Status", systemImage: "arrow.clockwise")
            }
            .disabled(model.isRefreshing || model.isBusy)
            .help("Read the configuration and installed files again. Does not check for newer catalog revisions.")
            .accessibilityIdentifier("health.refresh")

            Button {
                Task { await model.checkHealth() }
            } label: {
                Label("Check Health", systemImage: "stethoscope")
            }
            .disabled(model.isChecking || model.isBusy || !model.hasSavedConfig)
            .help("Look for missing prerequisites, changed files and ownership problems. Changes nothing.")
            .accessibilityIdentifier("health.check")

            if model.isRefreshing || model.isChecking {
                ProgressView().controlSize(.small)
            }

            Spacer()

            ReapplyButton(model: model)
        }
    }
}

private struct ReapplyButton: View {
    let model: HealthModel

    var body: some View {
        Button("Review and Reapply Configuration…") {
            Task { await model.reviewSavedConfiguration() }
        }
        .disabled(model.reapplyBlockedReason != nil)
        .help(
            model.reapplyBlockedReason
                ?? String(localized: "Review the saved configuration and apply it again to restore its installation."))
        .accessibilityIdentifier("health.reapply")
    }
}

private struct EnvironmentNote: View {
    var body: some View {
        Label {
            Text(
                "Checks inspect Ambit's own environment, which can differ from the environment your agent tools start with. A passing check does not prove a capability works when the tool runs it. Ambit never runs your shell startup scripts and never asks for capability secrets."
            )
            .fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: "info.circle")
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .accessibilityIdentifier("health.environmentNote")
    }
}

private struct SummaryRow: View {
    let summary: SetupHealthSummary
    let checkedAt: Date?
    let readAt: Date?

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(summary.title, systemImage: symbol)
                .font(.headline)
                .foregroundStyle(tint)
                .accessibilityIdentifier("health.summary")
            if let checkedAt {
                Text("Health checked \(checkedAt, format: .relative(presentation: .named)).")
                    .font(.caption).foregroundStyle(.secondary)
            } else if readAt != nil {
                Text("Status read from local files. Check health to look for missing prerequisites.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private var symbol: String {
        switch summary.level {
        case .installed: "checkmark.circle"
        case .setupRequired: "wrench.adjustable"
        case .notFullyInstalled: "exclamationmark.circle"
        case .needsAttention: "xmark.octagon"
        }
    }

    private var tint: Color {
        switch summary.level {
        case .installed: .green
        case .setupRequired: .blue
        case .notFullyInstalled: .orange
        case .needsAttention: .red
        }
    }
}

private struct ItemRow: View {
    let item: ItemHealth
    let model: HealthModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    Text(item.status.item.name).font(.body.weight(.medium))
                    Text(HealthModel.describe(item.status.item))
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                CategoryTag(category: item.category)
            }
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("health.item.\(item.status.item.catalog).\(item.status.item.name)")

            ForEach(item.status.artifacts.filter { $0.state != .ok }, id: \.path) { artifact in
                Text("\(artifact.path): \(artifact.state.title)")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
            }
        }
    }
}

private struct CategoryTag: View {
    let category: ItemCategory

    var body: some View {
        Text(category.title)
            .font(.caption.weight(.medium))
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(tint.opacity(0.12), in: Capsule())
    }

    private var tint: Color {
        switch category {
        case .installed: .green
        case .setupRequired: .blue
        case .selectedNotInstalled: .secondary
        case .notFullyInstalled, .drifted: .orange
        case .ownershipProblem: .red
        }
    }
}

private struct IssueView: View {
    let issue: HealthIssue
    let model: HealthModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(issue.title, systemImage: symbol)
                .font(.headline)
                .foregroundStyle(issue.severity == .error ? .red : .orange)
            if let affected = issue.affected {
                Text("Affects: \(affected)").font(.callout)
            }
            if !issue.explanation.isEmpty {
                Text(issue.explanation).fixedSize(horizontal: false, vertical: true)
            }
            Text("Next step: \(issue.nextStep)")
                .font(.callout.weight(.medium))
                .fixedSize(horizontal: false, vertical: true)

            if issue.action == .reviewAndReapply {
                ReapplyButton(model: model)
            }

            DisclosureGroup("Technical details") {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(issue.technical.enumerated()), id: \.offset) { _, line in
                        Text(line).font(.caption.monospaced()).textSelection(.enabled)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .padding(.vertical, 4)
        .accessibilityIdentifier("health.issue")
    }

    private var symbol: String {
        switch issue.kind {
        case .missingPrerequisite, .unresolvedReference: "key"
        case .lockOutdated, .drift: "arrow.triangle.2.circlepath"
        case .ownership: "lock.trianglebadge.exclamationmark"
        case .installMode: "info.circle"
        case .toolLimitation: "hammer"
        case .other: "exclamationmark.triangle"
        }
    }
}

extension ArtifactState {
    var title: String {
        switch self {
        case .ok: String(localized: "installed")
        case .missing: String(localized: "missing")
        case .modified: String(localized: "changed")
        case .unowned: String(localized: "not owned by Ambit")
        }
    }
}
