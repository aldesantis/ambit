import SwiftUI

struct CapabilitiesView: View {
    let setup: SetupModel
    @State private var model: CapabilitiesModel

    init(setup: SetupModel) {
        self.setup = setup
        _model = State(initialValue: CapabilitiesModel(setup: setup))
    }

    var body: some View {
        @Bindable var model = model

        Group {
            if model.canBrowse {
                HSplitView {
                    CapabilityListPane(model: model)
                        .frame(minWidth: 300, idealWidth: 380)
                    CapabilityDetailPane(model: model)
                        .frame(minWidth: 320, maxWidth: .infinity, maxHeight: .infinity)
                }
            } else {
                ContentUnavailableView(
                    "No Configuration", systemImage: "puzzlepiece.extension",
                    description: Text("Create or fix this setup's configuration to browse capabilities."))
            }
        }
        .task(id: setup.capabilitiesReloadKey) {
            await model.reload()
        }
        .sheet(item: $model.ruleDraft) { _ in
            RuleEditorSheet(model: model)
        }
        .sheet(item: $model.removal, onDismiss: model.removalDismissed) { _ in
            RemoveCapabilitySheet(model: model)
        }
        .alert(item: $model.actionError) { error in
            Alert(title: Text(error.title), message: Text(error.message))
        }
    }
}

/// Filters, warnings about catalogs and entries, and the item list.
private struct CapabilityListPane: View {
    @Bindable var model: CapabilitiesModel

    var body: some View {
        VStack(spacing: 0) {
            CapabilityFilterBar(model: model)
                .padding(10)
            Divider()

            List(selection: $model.selection) {
                if let error = model.loadError {
                    Section {
                        Label(error.message, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.red)
                    }
                }

                if !model.unavailableCatalogs.isEmpty {
                    Section("Catalogs Not Loaded") {
                        ForEach(model.unavailableCatalogs, id: \.name) { state in
                            CatalogStateRow(state: state)
                        }
                    }
                }

                if !model.problems.isEmpty || !model.unmatched.isEmpty {
                    Section("Selection Problems") {
                        ForEach(Array(model.problems.enumerated()), id: \.offset) { _, problem in
                            Label(problem.message, systemImage: "xmark.octagon")
                                .foregroundStyle(.red)
                        }
                        ForEach(model.unmatched, id: \.entry) { unmatched in
                            UnmatchedEntryRow(model: model, unmatched: unmatched)
                        }
                    }
                }

                if !model.rules.isEmpty {
                    Section("Selection Rules") {
                        ForEach(model.rules, id: \.self) { rule in
                            RuleRow(model: model, rule: rule)
                        }
                    }
                }

                Section {
                    ForEach(model.filteredItems, id: \.item) { item in
                        CapabilityRow(item: item)
                            .tag(item.item)
                    }
                } header: {
                    Text("\(model.filteredItems.count) of \(model.items.count) capabilities")
                }
            }
            .overlay {
                if model.hasLoaded, model.items.isEmpty, model.loadError == nil {
                    ContentUnavailableView(
                        "No Capabilities", systemImage: "puzzlepiece.extension",
                        description: Text(
                            model.configuredCatalogs.isEmpty
                                ? "Add a catalog in the Catalogs tab to browse its capabilities."
                                : "The configured catalogs offer nothing to browse yet."))
                } else if model.hasLoaded, !model.items.isEmpty, model.filteredItems.isEmpty {
                    ContentUnavailableView.search
                }
            }
            .accessibilityIdentifier("capabilities.list")
        }
    }
}

private struct CapabilityFilterBar: View {
    @Bindable var model: CapabilitiesModel

    var body: some View {
        VStack(spacing: 8) {
            HStack {
                TextField("Search names and descriptions", text: $model.searchText)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("capabilities.search")
                Button {
                    model.startNewRule()
                } label: {
                    Label("Add Selection Rule…", systemImage: "plus")
                }
                .disabled(model.configuredCatalogs.isEmpty)
                .help("Select every item whose name matches a pattern")
                .accessibilityIdentifier("capabilities.addRule")
            }

            HStack {
                Picker("Catalog", selection: $model.catalogFilter) {
                    Text("All Catalogs").tag(String?.none)
                    ForEach(model.configuredCatalogs, id: \.self) { name in
                        Text(name).tag(String?.some(name))
                    }
                }
                .accessibilityIdentifier("capabilities.filter.catalog")

                Picker("Kind", selection: $model.kindFilter) {
                    Text("All Kinds").tag(ItemKind?.none)
                    ForEach(ItemKind.allCases, id: \.self) { kind in
                        Text(kind.pluralTitle).tag(ItemKind?.some(kind))
                    }
                }
                .accessibilityIdentifier("capabilities.filter.kind")

                Picker("Status", selection: $model.selectionFilter) {
                    Text("All").tag(CapabilitiesModel.SelectionFilter.all)
                    Text("Selected").tag(CapabilitiesModel.SelectionFilter.selected)
                    Text("Unselected").tag(CapabilitiesModel.SelectionFilter.unselected)
                }
                .pickerStyle(.segmented)
                .accessibilityIdentifier("capabilities.filter.status")
            }
            .labelsHidden()
        }
    }
}

private struct CapabilityRow: View {
    let item: BrowseItem

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: item.item.kind.symbol)
                .foregroundStyle(item.selected ? Color.accentColor : .secondary)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(item.item.name)
                        .fontWeight(.medium)
                    Text("\(item.item.kind.title) · \(item.item.catalog)")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                if let description = item.description, !description.isEmpty {
                    Text(description)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
                let reasons = CapabilitiesModel.reasons(of: item)
                if !reasons.isEmpty {
                    HStack(spacing: 4) {
                        ForEach(reasons, id: \.self) { ReasonBadge(reason: $0) }
                    }
                }
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("capability.row.\(item.item.identifier)")
    }
}

private struct CatalogStateRow: View {
    let state: CatalogLoadState

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            switch state.availability {
            case .available:
                EmptyView()
            case .notCached:
                Label("\(state.name) is not downloaded yet", systemImage: "icloud.and.arrow.down")
                Text("Its capabilities appear here after you choose Load Catalogs in the Catalogs tab.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            case let .failed(error):
                Label("\(state.name) could not be loaded", systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.orange)
                Text(error.message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text("Check the catalog in the Catalogs tab.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityIdentifier("capabilities.catalogState.\(state.name)")
    }
}

private struct RuleRow: View {
    let model: CapabilitiesModel
    let rule: SelectionEntry

    var body: some View {
        HStack {
            Label(rule.displayText, systemImage: "asterisk.circle")
                .foregroundStyle(model.unmatchedError(for: rule) == nil ? Color.primary : .red)
            Spacer()
            Button("Edit…") { model.startEditing(rule) }
                .buttonStyle(.borderless)
            Button("Remove", role: .destructive) {
                Task { await model.removeEntry(rule) }
            }
            .buttonStyle(.borderless)
        }
        .accessibilityIdentifier("capabilities.rule.\(rule.displayText)")
    }
}

/// An entry that matches nothing. It blocks Apply, so it offers the fixes.
private struct UnmatchedEntryRow: View {
    let model: CapabilitiesModel
    let unmatched: UnmatchedEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(unmatched.entry.displayText, systemImage: "xmark.octagon")
                .foregroundStyle(.red)
            Text(unmatched.error.message)
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack {
                if unmatched.entry.isRule {
                    Button("Edit Rule…") { model.startEditing(unmatched.entry) }
                }
                Button("Remove Entry", role: .destructive) {
                    Task { await model.removeEntry(unmatched.entry) }
                }
            }
            .controlSize(.small)
        }
        .accessibilityIdentifier("capabilities.unmatched.\(unmatched.entry.displayText)")
    }
}
