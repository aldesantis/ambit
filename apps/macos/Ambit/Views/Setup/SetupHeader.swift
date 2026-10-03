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

/// Apply and Discard for the setup's draft.
private struct PendingChangesActions: View {
    let setup: SetupModel
    @State private var confirmingDiscard = false

    var body: some View {
        HStack {
            if setup.isDraftOutdated {
                Label("The file changed outside Ambit", systemImage: "exclamationmark.triangle")
                    .font(.callout)
                    .foregroundStyle(.orange)
                    .help("Refresh and reload the file to continue. Your changes can't be applied until then.")
            }
            Button("Discard Changes", role: .destructive) {
                confirmingDiscard = true
            }
            .accessibilityIdentifier("setup.discard")
            Button("Apply Changes…") {
                Task { await setup.startReview() }
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.return, modifiers: .command)
            .accessibilityIdentifier("setup.apply")
        }
        .disabled(setup.review != nil || setup.operations.isBusy)
        .confirmationDialog("Discard your changes to \(setup.displayName)?", isPresented: $confirmingDiscard) {
            Button("Discard Changes", role: .destructive) {
                setup.discardChanges()
            }
            .accessibilityIdentifier("setup.confirmDiscard")
        } message: {
            Text("Nothing has been written yet. The setup stays as it is on disk.")
        }
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
