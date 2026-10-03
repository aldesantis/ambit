import SwiftUI

/// What an apply or retry left behind, stated truthfully: what was saved, what was installed,
/// and that nothing is rolled back.
struct ApplyResultView: View {
    let outcome: ReviewModel.Outcome
    let mode: ReviewModel.Mode

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label(title, systemImage: symbol)
                .font(.title3.bold())
                .foregroundStyle(tint)
                .accessibilityIdentifier("review.result")
            content
        }
        .padding(20)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private var content: some View {
        switch outcome {
        case let .installed(summary):
            let count = summary.writes.count + summary.removals.count
            Text(
                count == 0
                    ? String(localized: "The setup is up to date.")
                    : String(localized: "\(count) managed files were written or removed."))
        case let .notFullyInstalled(saved, error):
            Text(
                saved
                    ? "The configuration was saved, but installing it failed. Files already installed were kept; nothing was rolled back."
                    : "Installing failed. Files already installed were kept; nothing was rolled back.")
            EngineErrorText(error: error)
            if case .ownershipConflict = error {
                Text(
                    "Ambit won't take over files it didn't record as its own, including ones a failed installation left behind. Move or delete them yourself, then retry."
                )
                .foregroundStyle(.secondary)
            }
        case let .saveFailed(error):
            Text("Nothing was saved or installed. Your changes are still in the draft.")
            EngineErrorText(error: error)
        case let .stale(error):
            Text(
                "The setup changed after this review, so applying it could do something different from what you reviewed. Nothing was saved. Review the changes again."
            )
            EngineErrorText(error: error)
        case .canceled:
            Text(mode == .apply ? "Canceled before any change was written. Your changes are still in the draft." : "Canceled before any change was written.")
        }
    }

    private var title: String {
        switch outcome {
        case .installed: String(localized: "Changes applied")
        case .notFullyInstalled: String(localized: "Changes not fully installed")
        case .saveFailed: String(localized: "Changes not saved")
        case .stale: String(localized: "The review is out of date")
        case .canceled: String(localized: "Canceled")
        }
    }

    private var symbol: String {
        switch outcome {
        case .installed: "checkmark.circle.fill"
        case .notFullyInstalled, .stale: "exclamationmark.triangle.fill"
        case .saveFailed: "xmark.octagon.fill"
        case .canceled: "xmark.circle"
        }
    }

    private var tint: Color {
        switch outcome {
        case .installed: .green
        case .notFullyInstalled, .stale: .orange
        case .saveFailed: .red
        case .canceled: .secondary
        }
    }
}
