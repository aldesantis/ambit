import SwiftUI

@main
struct AmbitApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model: AppModel

    init() {
        _model = State(initialValue: AppModel(environment: .make(for: .current)))
    }

    var body: some Scene {
        Window("Ambit", id: "main") {
            ContentView()
                .environment(model)
        }
        .defaultSize(width: 1080, height: 720)
        .commands {
            SidebarCommands()
            CommandGroup(replacing: .newItem) {
                Button("Add Project…") {
                    Task { await model.addProject() }
                }
                .keyboardShortcut("o")
            }
        }

        Settings {
            SettingsView()
                .environment(model)
        }
    }
}
