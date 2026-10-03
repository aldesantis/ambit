import SwiftUI

/// The selected item: what selects it, what it needs, and its own content, read-only.
struct CapabilityDetailPane: View {
    let model: CapabilitiesModel

    var body: some View {
        Group {
            if let item = model.selectedItem {
                ScrollView {
                    CapabilityDetail(model: model, item: item)
                        .padding(20)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                ContentUnavailableView(
                    "No Capability Selected", systemImage: "puzzlepiece.extension",
                    description: Text("Choose a skill, pack, MCP server or hook to see its details."))
            }
        }
        .task(id: model.selection) {
            await model.loadDetail()
        }
    }
}

private struct CapabilityDetail: View {
    let model: CapabilitiesModel
    let item: BrowseItem

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            header

            DetailSection("Selection") {
                if item.routes.isEmpty {
                    Text("Not selected.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(Array(item.routes.enumerated()), id: \.offset) { _, route in
                        HStack(alignment: .firstTextBaseline, spacing: 6) {
                            ReasonBadge(reason: .init(route))
                            Text(route.explanation)
                                .textSelection(.enabled)
                        }
                    }
                    if item.routes.count > 1 {
                        Text("It stays installed while any of these selects it.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }

            if !item.requires.isEmpty {
                DetailSection("Dependencies") {
                    ForEach(item.requires, id: \.self) { entry in
                        Text(entry.displayText)
                            .font(.system(.body, design: .monospaced))
                    }
                }
            }

            if !item.expects.isEmpty {
                DetailSection("Prerequisites") {
                    ForEach(item.expects, id: \.self) { expectation in
                        Label(expectation, systemImage: "checklist")
                    }
                }
            }

            if !item.limitations.isEmpty {
                DetailSection("Agent Tool Limitations") {
                    ForEach(item.limitations, id: \.self) { limitation in
                        Label(limitation.message, systemImage: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                    }
                }
            }

            content

            if let error = model.detailError {
                Label(error.message, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
            }
        }
    }

    private var header: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 4) {
                Label(item.item.name, systemImage: item.item.kind.symbol)
                    .font(.title2.bold())
                    .accessibilityAddTraits(.isHeader)
                    .accessibilityIdentifier("capability.name")
                Text("\(item.item.kind.title) from \(item.item.catalog)")
                    .foregroundStyle(.secondary)
                if let description = item.description, !description.isEmpty {
                    Text(description)
                        .textSelection(.enabled)
                        .padding(.top, 4)
                }
            }
            Spacer()
            actions
        }
    }

    @ViewBuilder
    private var actions: some View {
        VStack(alignment: .trailing, spacing: 6) {
            if !model.isDirectlySelected(item) {
                Button {
                    Task { await model.select(item.item) }
                } label: {
                    Label("Select", systemImage: "plus.circle")
                }
                .buttonStyle(.borderedProminent)
                .help("Add an entry that selects this item")
                .accessibilityIdentifier("capability.select")
            }
            if item.selected {
                Button(role: .destructive) {
                    Task { await model.requestRemoval(of: item.item) }
                } label: {
                    Label("Remove…", systemImage: "minus.circle")
                }
                .help("Remove the selections that keep this item installed")
                .accessibilityIdentifier("capability.remove")
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch item.detail {
        case .skill:
            DetailSection("SKILL.md") {
                if let document = model.skillDocument {
                    SkillDocumentView(document: document)
                } else if model.detailError == nil {
                    ProgressView().controlSize(.small)
                }
            }
        case let .pack(requires):
            DetailSection("Pack Entries") {
                ForEach(requires, id: \.self) { entry in
                    Text(entry.displayText)
                        .font(.system(.body, design: .monospaced))
                }
            }
            DetailSection("Resolved Contents") {
                if let contents = model.packContents {
                    if contents.isEmpty {
                        Text("Nothing.").foregroundStyle(.secondary)
                    }
                    ForEach(contents, id: \.self) { ref in
                        Label("\(ref.address)  (\(ref.kind.title))", systemImage: ref.kind.symbol)
                    }
                } else if model.detailError == nil {
                    ProgressView().controlSize(.small)
                }
            }
        case let .mcp(transport):
            DetailSection("Connection") {
                McpTransportView(transport: transport)
            }
        case let .hook(event, matcher, hookType, command, timeout):
            DetailSection("Hook") {
                FieldGrid(fields: [
                    (String(localized: "Event"), event),
                    (String(localized: "Matcher"), matcher ?? String(localized: "Any")),
                    (String(localized: "Type"), hookType),
                    (String(localized: "Command"), command ?? "–"),
                    (String(localized: "Timeout"), timeout.map { String(localized: "\($0) seconds") } ?? String(localized: "Default")),
                ])
                Text("Ambit never runs hook commands while browsing.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

private struct McpTransportView: View {
    let transport: McpTransport

    var body: some View {
        switch transport {
        case let .stdio(command, args, envNames):
            FieldGrid(fields: [
                (String(localized: "Transport"), String(localized: "Local process (stdio)")),
                (String(localized: "Command"), command),
                (String(localized: "Arguments"), args.isEmpty ? "–" : args.joined(separator: " ")),
                (String(localized: "Environment"), envNames.isEmpty ? "–" : envNames.joined(separator: ", ")),
            ])
        case let .http(url, headerNames):
            FieldGrid(fields: [
                (String(localized: "Transport"), String(localized: "HTTP")),
                (String(localized: "URL"), url),
                (String(localized: "Headers"), headerNames.isEmpty ? "–" : headerNames.joined(separator: ", ")),
            ])
        }
    }
}

private struct FieldGrid: View {
    let fields: [(String, String)]

    var body: some View {
        Grid(alignment: .leadingFirstTextBaseline, horizontalSpacing: 12, verticalSpacing: 6) {
            ForEach(Array(fields.enumerated()), id: \.offset) { _, field in
                GridRow {
                    Text(field.0)
                        .foregroundStyle(.secondary)
                        .gridColumnAlignment(.trailing)
                    Text(verbatim: field.1)
                        .font(.system(.body, design: .monospaced))
                        .textSelection(.enabled)
                }
            }
        }
    }
}

private struct DetailSection<Content: View>: View {
    let title: LocalizedStringKey
    @ViewBuilder let content: Content

    init(_ title: LocalizedStringKey, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.headline)
            content
        }
    }
}
