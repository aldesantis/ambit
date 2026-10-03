import SwiftUI

/// Explains every selection that keeps an item installed and lets the user remove each one
/// explicitly. Nothing is removed in cascade, and the item is described as uninstalled only once
/// no selection reaches it.
struct RemoveCapabilitySheet: View {
    @Bindable var model: CapabilitiesModel

    var body: some View {
        if let plan = model.removal {
            VStack(alignment: .leading, spacing: 16) {
                Text("Remove \(plan.item.name)")
                    .font(.title3.bold())

                if plan.isComplete {
                    Label(
                        "No selection keeps \(plan.item.address) any more. Apply the changes to uninstall it.",
                        systemImage: "checkmark.circle"
                    )
                    .foregroundStyle(.green)
                    .accessibilityIdentifier("removal.complete")
                } else {
                    Text(
                        "\(plan.item.address) stays installed while any of these selections reaches it. Remove or edit each one you no longer want."
                    )
                    .fixedSize(horizontal: false, vertical: true)

                    ScrollView {
                        VStack(alignment: .leading, spacing: 12) {
                            ForEach(plan.steps) { step in
                                StepView(model: model, plan: plan, step: step)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .frame(maxHeight: 360)

                    if !plan.removed.isEmpty, plan.steps.count > 1 {
                        AffectedList(
                            title: "Removing all of them would uninstall:", items: plan.removed)
                    }
                }

                if !plan.staged.isEmpty {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Staged in this draft:")
                            .font(.caption.bold())
                        ForEach(plan.staged, id: \.self) { entry in
                            Text("Remove \(entry.displayText)")
                                .font(.caption)
                        }
                    }
                    .foregroundStyle(.secondary)
                }

                HStack {
                    Spacer()
                    Button("Done") { model.removal = nil }
                        .keyboardShortcut(.defaultAction)
                        .accessibilityIdentifier("removal.done")
                }
            }
            .padding(20)
            .frame(width: 520)
        }
    }
}

private struct StepView: View {
    let model: CapabilitiesModel
    let plan: CapabilitiesModel.RemovalPlan
    let step: CapabilitiesModel.RemovalStep

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                ReasonBadge(reason: step.reason)
                Text(step.entry.displayText)
                    .font(.system(.body, design: .monospaced))
            }

            Text(explanation)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(Array(step.routes.enumerated()), id: \.offset) { _, route in
                Text(route.explanation)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            AffectedList(title: "Removing this entry would uninstall:", items: step.removes)

            HStack {
                if step.reason == .rule {
                    Button("Edit Rule…") {
                        model.editRuleAfterRemoval(step.entry)
                    }
                }
                Button(removeTitle, role: .destructive) {
                    Task { await model.removeStep(step) }
                }
                .accessibilityIdentifier("removal.remove.\(step.entry.displayText)")
            }
            .controlSize(.small)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(.quaternary.opacity(0.4), in: RoundedRectangle(cornerRadius: 8))
    }

    private var explanation: String {
        let item = plan.item.address
        switch step.reason {
        case .direct:
            return String(localized: "This entry selects \(item) directly.")
        case .rule:
            return String(
                localized:
                    "This rule matches \(item). Edit the pattern so it no longer matches, or remove the rule and everything it selects.")
        case .pack:
            return String(
                localized:
                    "\(item) is part of the pack this entry selects. Packs are not edited here; removing the pack entry removes the whole pack.")
        case .dependency:
            return String(
                localized:
                    "\(item) is required by a capability this entry selects. Removing \(item) means removing this selection too.")
        }
    }

    private var removeTitle: String {
        switch step.reason {
        case .direct: String(localized: "Remove Entry")
        case .rule: String(localized: "Remove Rule")
        case .pack: String(localized: "Remove Pack Entry")
        case .dependency: String(localized: "Remove This Selection")
        }
    }
}

private struct AffectedList: View {
    let title: LocalizedStringKey
    let items: [ItemRef]

    var body: some View {
        if !items.isEmpty {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                    .font(.caption.bold())
                ForEach(items, id: \.self) { ref in
                    Label("\(ref.address)  (\(ref.kind.title))", systemImage: ref.kind.symbol)
                        .font(.caption)
                }
            }
        }
    }
}
