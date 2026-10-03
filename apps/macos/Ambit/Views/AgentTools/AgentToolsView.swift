import SwiftUI

struct AgentToolsView: View {
    let setup: SetupModel

    var body: some View {
        let model = setup.agentTools

        Form {
            Section {
                Text(
                    "Choose the agent tools this setup installs capabilities for. A tool doesn't need to be installed on this Mac to be configured. Changes are staged until you review and apply them."
                )
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

                if !model.canEdit {
                    Label(
                        "This setup has no configuration yet. Create one to choose its agent tools.",
                        systemImage: "info.circle"
                    )
                    .accessibilityIdentifier("agentTools.unconfigured")
                }
                if let refusal = model.refusal {
                    Label(refusal, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.orange)
                        .accessibilityIdentifier("agentTools.refusal")
                }
                if let error = model.editError {
                    Label(error.message, systemImage: "xmark.octagon")
                        .foregroundStyle(.red)
                        .accessibilityIdentifier("agentTools.error")
                }
            }

            ForEach(model.tools) { tool in
                AgentToolSection(model: model, tool: tool)
            }

            if !model.unknownSelected.isEmpty {
                Section("Other tools in the configuration") {
                    ForEach(model.unknownSelected, id: \.self) { name in
                        Label(
                            "\(name) is not an agent tool this version of Ambit supports. It is kept as it is.",
                            systemImage: "questionmark.circle")
                    }
                }
            }
        }
        .formStyle(.grouped)
        .disabled(setup.isApplying)
    }
}

private struct AgentToolSection: View {
    let model: AgentToolsModel
    let tool: AgentToolInfo
    @State private var expanded = false

    var body: some View {
        Section {
            HStack {
                Toggle(isOn: binding) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(tool.displayName).font(.headline)
                        if model.isLastSelected(tool.id) {
                            Text("At least one agent tool is required.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                .disabled(!model.canEdit)
                .accessibilityIdentifier("agentTools.toggle.\(tool.id)")
                .accessibilityHint(
                    model.isLastSelected(tool.id)
                        ? Text("The only selected agent tool. Turn on another tool first.") : Text(""))

                switch model.change(for: tool.id) {
                case .added:
                    ChangeTag(text: "Will be added", tint: .blue)
                case .removed:
                    ChangeTag(text: "Will be removed", tint: .orange)
                case .unchanged:
                    EmptyView()
                }
            }

            DisclosureGroup("Files and limitations", isExpanded: $expanded) {
                VStack(alignment: .leading, spacing: 10) {
                    Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 4) {
                        ForEach(model.writtenFiles(for: tool)) { file in
                            GridRow {
                                Text(file.purpose).foregroundStyle(.secondary)
                                Text(file.path)
                                    .font(.body.monospaced())
                                    .textSelection(.enabled)
                            }
                            .accessibilityElement(children: .combine)
                        }
                    }
                    if tool.hooksFile == nil {
                        Text("No hooks file: hooks you select are skipped for \(tool.displayName).")
                            .foregroundStyle(.secondary)
                    }

                    if !tool.limitations.isEmpty {
                        VStack(alignment: .leading, spacing: 4) {
                            Text("Limitations").font(.subheadline.weight(.semibold))
                            ForEach(tool.limitations, id: \.self) { limitation in
                                Label(limitation, systemImage: "exclamationmark.triangle")
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                        .accessibilityIdentifier("agentTools.limitations.\(tool.id)")
                    }

                    Text(model.scopeNote(for: tool))
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .padding(.top, 4)
            }
            .accessibilityIdentifier("agentTools.details.\(tool.id)")
        }
    }

    private var binding: Binding<Bool> {
        Binding(
            get: { model.isSelected(tool.id) },
            set: { model.setSelected(tool.id, $0) })
    }
}

private struct ChangeTag: View {
    let text: LocalizedStringKey
    let tint: Color

    var body: some View {
        Text(text)
            .font(.caption.weight(.medium))
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 3)
            .background(tint.opacity(0.12), in: Capsule())
    }
}
