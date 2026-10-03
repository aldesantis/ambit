import SwiftUI

/// The stage and progress of a review or apply. Cancel is offered only until the engine starts
/// writing, because stopping mid-write would leave a partial install rather than roll back.
struct ApplyProgressView: View {
    let title: LocalizedStringKey
    let operation: OperationRunner.Operation?
    let canCancel: Bool
    let cancel: () -> Void

    var body: some View {
        VStack(spacing: 16) {
            Text(title)
                .font(.title3.bold())

            if let progress = operation?.progress, progress.total > 0 {
                ProgressView(value: Double(progress.current), total: Double(progress.total))
                    .frame(maxWidth: 360)
            } else {
                ProgressView()
            }

            VStack(spacing: 4) {
                Text(operation?.progress?.stage.title ?? String(localized: "Waiting for another operation…"))
                    .accessibilityIdentifier("progress.stage")
                if let subject = operation?.progress?.subject, !subject.isEmpty {
                    Text(subject)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }

            if canCancel {
                Button("Cancel", role: .cancel, action: cancel)
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("progress.cancel")
            } else {
                Text("Writing files. This can't be canceled.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("review.progress")
    }
}

extension Stage {
    var title: String {
        switch self {
        case .loadingCatalogs: String(localized: "Loading catalogs")
        case .fetching: String(localized: "Fetching catalogs")
        case .resolving: String(localized: "Resolving capabilities")
        case .planning: String(localized: "Planning changes")
        case .checkingOwnership: String(localized: "Checking existing files")
        case .savingConfig: String(localized: "Saving the configuration")
        case .writingFiles: String(localized: "Writing files")
        case .removingFiles: String(localized: "Removing files")
        case .writingRecords: String(localized: "Recording the installation")
        }
    }
}
