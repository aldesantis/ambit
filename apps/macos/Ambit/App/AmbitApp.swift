import SwiftUI

@main
struct AmbitApp: App {
    static let mainWindowID = "main"

    @NSApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model: AppModel
    @State private var lifecycle: AppLifecycle

    init() {
        let environment = AppEnvironment.make(for: .current)
        let model = AppModel(environment: environment)
        let lifecycle = AppLifecycle(environment: environment)
        lifecycle.pendingChanges = model
        _model = State(initialValue: model)
        _lifecycle = State(initialValue: lifecycle)
        AppDelegate.lifecycle = lifecycle
    }

    var body: some Scene {
        Window("Ambit", id: Self.mainWindowID) {
            ContentView()
                .environment(model)
                .environment(lifecycle)
                .background(WindowAccessor { window in appDelegate.attach(mainWindow: window) })
        }
        .defaultSize(width: 1080, height: 720)
        .restorationBehavior(.disabled)
        .defaultLaunchBehavior(LaunchContext.current.isLoginLaunch ? .suppressed : .presented)
        // A normal launch always opens the window and a login launch starts in the menu bar only,
        // so whether the window was open when the app last quit must not matter.
        .commands {
            SidebarCommands()
            CommandGroup(replacing: .newItem) {
                Button("Add Project…") {
                    Task { await model.addProject() }
                }
                .keyboardShortcut("o")
            }
            CommandGroup(after: .appInfo) {
                Button("Check for App Updates…") {
                    lifecycle.updater.checkForUpdates()
                }
                .disabled(!lifecycle.updater.state.canCheck)
            }
        }

        MenuBarExtra {
            MenuBarContent()
                .environment(lifecycle)
        } label: {
            MenuBarLabel()
                .environment(lifecycle)
        }
        .menuBarExtraStyle(.menu)

        Settings {
            SettingsView()
                .environment(model)
                .environment(lifecycle)
        }
    }
}
