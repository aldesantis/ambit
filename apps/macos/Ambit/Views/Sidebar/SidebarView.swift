import SwiftUI

struct SidebarView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        List(selection: Binding(get: { model.selection }, set: { model.select($0) })) {
            Section("Personal") {
                Label("Personal setup", systemImage: "person.crop.circle")
                    .tag(SetupID.personal)
                    .accessibilityIdentifier("sidebar.personal")
            }

            Section("Projects") {
                ForEach(model.projects) { project in
                    ProjectRow(project: project, isAvailable: model.isAvailable(project))
                        .tag(SetupID.project(path: project.path))
                        .contextMenu { menu(for: project) }
                }
            }
        }
        .listStyle(.sidebar)
        .safeAreaInset(edge: .bottom, spacing: 0) {
            footer
        }
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 4) {
            Button {
                Task { await model.addProject() }
            } label: {
                Label("Add Project…", systemImage: "plus")
            }
            .accessibilityIdentifier("sidebar.addProject")
            .help("Add a project folder (⌘O)")

            SettingsLink {
                Label("Settings", systemImage: "gearshape")
            }
            .accessibilityIdentifier("sidebar.settings")
        }
        .buttonStyle(.borderless)
        .labelStyle(.titleAndIcon)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.vertical, 12)
    }

    @ViewBuilder
    private func menu(for project: RememberedProject) -> some View {
        if model.isAvailable(project) {
            Button("Show in Finder") {
                NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: project.path)])
            }
        } else {
            Button("Locate Folder…") {
                Task { await model.locate(project) }
            }
        }
        Divider()
        Button("Forget Project") {
            model.forget(project)
        }
    }
}

private struct ProjectRow: View {
    let project: RememberedProject
    let isAvailable: Bool

    var body: some View {
        Label {
            VStack(alignment: .leading, spacing: 1) {
                Text(project.name)
                Text(project.path)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
        } icon: {
            Image(systemName: isAvailable ? "folder" : "exclamationmark.triangle")
                .foregroundStyle(isAvailable ? AnyShapeStyle(.tint) : AnyShapeStyle(.orange))
        }
        .help(project.path)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(project.name)
        .accessibilityValue(isAvailable ? project.path : String(localized: "Folder unavailable, \(project.path)"))
    }
}
