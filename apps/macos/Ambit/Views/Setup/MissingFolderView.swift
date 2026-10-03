import SwiftUI

struct MissingFolderView: View {
    @Environment(AppModel.self) private var model
    let project: RememberedProject

    var body: some View {
        ContentUnavailableView {
            Label("Folder Unavailable", systemImage: "folder.badge.questionmark")
        } description: {
            Text("Ambit can't find \(project.path). It may have been moved, renamed or deleted.")
        } actions: {
            Button("Locate Folder…") {
                Task { await model.locate(project) }
            }
            .buttonStyle(.borderedProminent)
            .accessibilityIdentifier("missing.locate")

            Button("Forget Project") {
                Task { await model.forget(project) }
            }
            .accessibilityIdentifier("missing.forget")
        }
        .navigationTitle(project.name)
        .navigationSubtitle(project.path)
    }
}
