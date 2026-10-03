import SwiftUI

extension View {
    /// Shows `AppModel.pendingPrompt` as an Apply / Discard / Cancel alert.
    func pendingChangesAlert(_ model: AppModel) -> some View {
        modifier(PendingChangesAlert(model: model))
    }
}

private struct PendingChangesAlert: ViewModifier {
    let model: AppModel

    func body(content: Content) -> some View {
        let prompt = model.pendingPrompt

        content.alert(
            prompt.map { "Apply your changes to \($0.setupName)?" } ?? "",
            isPresented: Binding(
                get: { model.pendingPrompt != nil },
                set: { if !$0 { model.answerPendingPrompt(.cancel) } }),
            presenting: prompt
        ) { _ in
            Button("Apply…") {
                model.answerPendingPrompt(.apply)
            }
            .keyboardShortcut(.defaultAction)
            .accessibilityIdentifier("pending.apply")
            Button("Discard Changes", role: .destructive) {
                model.answerPendingPrompt(.discard)
            }
            .accessibilityIdentifier("pending.discard")
            Button("Cancel", role: .cancel) {
                model.answerPendingPrompt(.cancel)
            }
            .accessibilityIdentifier("pending.cancel")
        } message: { prompt in
            Text(Self.message(for: prompt.reason))
        }
    }

    private static func message(for reason: AppModel.PendingChangeReason) -> String {
        switch reason {
        case .switchSetup:
            String(localized: "You have changes that haven't been applied. Apply them before switching setups, or discard them.")
        case .forgetProject:
            String(localized: "You have changes that haven't been applied. Apply them before forgetting the project, or discard them.")
        case .closeWindow:
            String(localized: "You have changes that haven't been applied. Apply them before closing the window, or discard them.")
        case .quit:
            String(localized: "You have changes that haven't been applied. Apply them before quitting, or discard them.")
        case .restartToUpdate:
            String(
                localized: "You have changes that haven't been applied. Apply them before restarting to update, or discard them. Cancel keeps them and postpones the update."
            )
        }
    }
}
