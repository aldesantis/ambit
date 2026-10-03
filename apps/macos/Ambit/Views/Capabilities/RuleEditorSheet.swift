import SwiftUI

/// Adds or edits a selection rule, previewing its matches live with the engine's validation.
struct RuleEditorSheet: View {
    @Bindable var model: CapabilitiesModel

    var body: some View {
        if let draft = model.ruleDraft {
            VStack(alignment: .leading, spacing: 16) {
                Text(draft.original == nil ? "Add Selection Rule" : "Edit Selection Rule")
                    .font(.title3.bold())

                Form {
                    Picker("Catalog", selection: binding(\.catalog)) {
                        ForEach(model.configuredCatalogs, id: \.self) { Text($0).tag($0) }
                    }
                    .accessibilityIdentifier("rule.catalog")
                    Picker("Kind", selection: binding(\.kind)) {
                        ForEach(ItemKind.allCases, id: \.self) { Text($0.title).tag($0) }
                    }
                    .accessibilityIdentifier("rule.kind")
                    TextField("Pattern", text: binding(\.pattern), prompt: Text("review-*"))
                        .accessibilityIdentifier("rule.pattern")
                }
                .formStyle(.grouped)

                preview(draft.preview)
                    .frame(maxWidth: .infinity, minHeight: 120, alignment: .topLeading)

                Text("A rule selects whatever matches when you apply. Future catalog updates can change its matches.")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                HStack {
                    Spacer()
                    Button("Cancel", role: .cancel) { model.ruleDraft = nil }
                        .keyboardShortcut(.cancelAction)
                    Button(draft.original == nil ? "Add Rule" : "Save Rule") {
                        Task { await model.saveRuleDraft() }
                    }
                    .keyboardShortcut(.defaultAction)
                    .disabled(!draft.canSave)
                    .accessibilityIdentifier("rule.save")
                }
            }
            .padding(20)
            .frame(width: 480)
            .task(id: draft.entry) {
                try? await Task.sleep(for: .milliseconds(250))
                guard !Task.isCancelled else {
                    return
                }
                await model.previewRuleDraft()
            }
        }
    }

    @ViewBuilder
    private func preview(_ preview: CapabilitiesModel.RuleDraft.Preview) -> some View {
        switch preview {
        case .idle:
            Text("Enter a pattern. Use * to match any part of a name.")
                .foregroundStyle(.secondary)
        case .loading:
            ProgressView().controlSize(.small)
        case let .matches(found):
            VStack(alignment: .leading, spacing: 4) {
                Text("Matches \(found.count) now:")
                    .font(.headline)
                ScrollView {
                    VStack(alignment: .leading, spacing: 2) {
                        ForEach(found, id: \.self) { ref in
                            Label(ref.address, systemImage: ref.kind.symbol)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 160)
            }
            .accessibilityIdentifier("rule.matches")
        case let .invalid(error):
            VStack(alignment: .leading, spacing: 4) {
                Label(error.message, systemImage: "xmark.octagon")
                    .foregroundStyle(.red)
                ForEach(error.detail, id: \.self) { line in
                    Text(line).font(.caption).foregroundStyle(.secondary)
                }
            }
            .accessibilityIdentifier("rule.error")
        }
    }

    private func binding<Value>(_ keyPath: WritableKeyPath<CapabilitiesModel.RuleDraft, Value>) -> Binding<Value> {
        Binding {
            model.ruleDraft![keyPath: keyPath]
        } set: { value in
            model.ruleDraft?[keyPath: keyPath] = value
        }
    }
}
