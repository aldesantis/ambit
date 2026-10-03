import SwiftUI

/// Creates a config for a root without one: choose agent tools, add a catalog or skip, choose
/// capabilities, then review and apply. Everything stays in the draft until Apply, so Cancel
/// leaves the root untouched. The catalog and capability steps host the Catalogs and
/// Capabilities area views, which edit the same draft.
struct NewSetupFlow: View {
    let setup: SetupModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            StepIndicator(step: setup.newSetupStep)

            switch setup.newSetupStep {
            case .tools:
                ToolsStep(setup: setup)
            case .catalog:
                AreaStep(
                    title: "Add a catalog",
                    explanation:
                        "Catalogs are the local folders and Git repositories capabilities come from. Add one now, or skip this step to create an empty setup.",
                    setup: setup
                ) {
                    CatalogsView(setup: setup)
                }
            case .capabilities:
                AreaStep(
                    title: "Choose capabilities",
                    explanation: "Select the skills, packs, MCP servers and hooks to install, or add a selection rule.",
                    setup: setup
                ) {
                    CapabilitiesView(setup: setup)
                }
            }
        }
        .padding([.horizontal, .bottom], 20)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

private struct StepIndicator: View {
    let step: SetupModel.NewSetupStep

    var body: some View {
        HStack(spacing: 8) {
            item(1, "Agent tools", active: step == .tools)
            Image(systemName: "chevron.right").foregroundStyle(.tertiary)
            item(2, "Catalog", active: step == .catalog)
            Image(systemName: "chevron.right").foregroundStyle(.tertiary)
            item(3, "Capabilities", active: step == .capabilities)
            Image(systemName: "chevron.right").foregroundStyle(.tertiary)
            item(4, "Review", active: false)
        }
        .font(.callout)
        .accessibilityElement(children: .combine)
    }

    private func item(_ number: Int, _ title: LocalizedStringKey, active: Bool) -> some View {
        HStack(spacing: 4) {
            Image(systemName: "\(number).circle\(active ? ".fill" : "")")
            Text(title)
        }
        .foregroundStyle(active ? AnyShapeStyle(.tint) : AnyShapeStyle(.secondary))
        .fontWeight(active ? .semibold : .regular)
    }
}

private struct ToolsStep: View {
    let setup: SetupModel

    var body: some View {
        let tools = setup.engine.supportedAgentTools()

        VStack(alignment: .leading, spacing: 12) {
            Text("This setup isn't configured yet")
                .font(.title3.bold())
                .accessibilityAddTraits(.isHeader)
            Text(
                "Choose the agent tools to install capabilities for. Choose at least one. The tools don't need to be installed yet."
            )
            .foregroundStyle(.secondary)
            if setup.id == .personal {
                Text(
                    "Personal setup applies to all your work. Each agent tool decides how it combines with a project's setup."
                )
                .foregroundStyle(.secondary)
            }

            VStack(alignment: .leading, spacing: 8) {
                ForEach(tools) { tool in
                    Toggle(isOn: binding(for: tool.id)) {
                        Text(tool.displayName)
                    }
                    .toggleStyle(.checkbox)
                    .accessibilityIdentifier("newSetup.tool.\(tool.id)")
                }
            }
            .padding(.vertical, 4)

            HStack {
                Button("Continue") {
                    continueWith(tools)
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(setup.newSetupTools.isEmpty)
                .accessibilityIdentifier("newSetup.continue")
                if setup.draft != nil {
                    Button("Cancel") {
                        setup.discardChanges()
                    }
                    .accessibilityIdentifier("newSetup.cancel")
                }
            }
        }
        .frame(maxWidth: 560, alignment: .leading)
    }

    private func binding(for id: String) -> Binding<Bool> {
        Binding(
            get: { setup.newSetupTools.contains(id) },
            set: { checked in
                if checked {
                    setup.newSetupTools.insert(id)
                } else {
                    setup.newSetupTools.remove(id)
                }
            })
    }

    private func continueWith(_ tools: [AgentToolInfo]) {
        let harnesses = tools.map(\.id).filter(setup.newSetupTools.contains)
        do {
            try setup.startNewSetup(harnesses: harnesses)
            setup.newSetupStep = .catalog
        } catch {
            setup.presentedError = PresentedError(title: String(localized: "Could not start the setup."), error: error)
        }
    }
}

/// A step that hosts an area view, with Back, Cancel and the way forward.
private struct AreaStep<Area: View>: View {
    let title: LocalizedStringKey
    let explanation: LocalizedStringKey
    let setup: SetupModel
    @ViewBuilder let area: () -> Area

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title)
                .font(.title3.bold())
                .accessibilityAddTraits(.isHeader)
            Text(explanation)
                .foregroundStyle(.secondary)

            area()
                .frame(maxWidth: .infinity, maxHeight: .infinity)

            HStack {
                Button("Back") {
                    setup.newSetupStep = setup.newSetupStep == .capabilities ? .catalog : .tools
                }
                .accessibilityIdentifier("newSetup.back")
                Button("Cancel") {
                    setup.discardChanges()
                }
                .accessibilityIdentifier("newSetup.cancel")
                Spacer()
                forward
            }
        }
    }

    @ViewBuilder
    private var forward: some View {
        let hasCatalogs = !(setup.configSummary?.catalogs.isEmpty ?? true)

        if setup.newSetupStep == .catalog && !hasCatalogs {
            Button("Skip and Review…") {
                Task { await setup.startReview() }
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .disabled(setup.operations.isBusy)
            .accessibilityIdentifier("newSetup.skip")
        } else if setup.newSetupStep == .catalog {
            Button("Continue") {
                setup.newSetupStep = .capabilities
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .accessibilityIdentifier("newSetup.continue")
        } else {
            Button("Review…") {
                Task { await setup.startReview() }
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
            .disabled(setup.operations.isBusy)
            .accessibilityIdentifier("newSetup.review")
        }
    }
}
