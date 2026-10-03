import SwiftUI

struct SetupHeader: View {
    let setup: SetupModel

    var body: some View {
        HStack(alignment: .center, spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 10) {
                    Text(setup.displayName)
                        .font(.title2.bold())
                        .accessibilityAddTraits(.isHeader)
                        .accessibilityIdentifier("setup.name")
                    if let badge = setup.badge {
                        SetupBadgeView(badge: badge)
                    }
                }
                Text(setup.root.path)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .accessibilityLabel("Folder \(setup.root.path)")
                    .accessibilityIdentifier("setup.root")
            }

            Spacer()

            if setup.hasPendingChanges {
                PendingChangesActions(setup: setup)
            }
        }
    }
}

/// Discard and Review for the setup's draft. The draft model provides the actions.
private struct PendingChangesActions: View {
    let setup: SetupModel

    var body: some View {
        HStack {
            Button("Discard Changes", role: .destructive) {}
            Button("Review Changes…") {}
                .buttonStyle(.borderedProminent)
        }
        .disabled(true)
    }
}

struct SetupBadgeView: View {
    let badge: SetupModel.Badge

    var body: some View {
        Label(title, systemImage: symbol)
            .font(.caption.weight(.medium))
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(tint.opacity(0.12), in: Capsule())
            .accessibilityLabel("Status: \(title)")
            .accessibilityIdentifier("setup.badge")
    }

    private var title: String {
        switch badge {
        case .unconfigured: String(localized: "Not configured")
        case .pendingChanges: String(localized: "Pending changes")
        case .installing: String(localized: "Installing")
        case .installed: String(localized: "Installed")
        case .notFullyInstalled: String(localized: "Not fully installed")
        case .folderUnavailable: String(localized: "Folder unavailable")
        case .error: String(localized: "Needs attention")
        }
    }

    private var symbol: String {
        switch badge {
        case .unconfigured: "circle.dashed"
        case .pendingChanges: "pencil.circle"
        case .installing: "arrow.down.circle"
        case .installed: "checkmark.circle"
        case .notFullyInstalled: "exclamationmark.circle"
        case .folderUnavailable: "folder.badge.questionmark"
        case .error: "xmark.octagon"
        }
    }

    private var tint: Color {
        switch badge {
        case .unconfigured: .secondary
        case .pendingChanges, .installing: .blue
        case .installed: .green
        case .notFullyInstalled, .folderUnavailable: .orange
        case .error: .red
        }
    }
}
