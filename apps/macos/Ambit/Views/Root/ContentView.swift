import SwiftUI

struct ContentView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        @Bindable var model = model

        NavigationSplitView {
            SidebarView()
                .navigationSplitViewColumnWidth(min: 200, ideal: 240, max: 360)
        } detail: {
            detail
                .id(model.selection)
        }
        .alert(item: $model.presentedError) { error in
            Alert(title: Text(error.title), message: Text(error.message))
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch model.selection {
        case nil:
            ContentUnavailableView("No Setup Selected", systemImage: "sidebar.left")
        case .personal:
            SetupDetailView(setup: model.setupModel(for: .personal))
        case let .project(path):
            if let project = model.project(at: path) {
                if model.isAvailable(project) {
                    SetupDetailView(setup: model.setupModel(for: .project(path: path)))
                } else {
                    MissingFolderView(project: project)
                }
            } else {
                ContentUnavailableView("No Setup Selected", systemImage: "sidebar.left")
            }
        }
    }
}
