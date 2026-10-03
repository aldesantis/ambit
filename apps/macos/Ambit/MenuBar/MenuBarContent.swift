// The menu bar presence: Open Ambit, app update status with the actions that apply, and Quit.
// Catalogs are never checked from here.

import SwiftUI

struct MenuBarContent: View {
    @Environment(AppLifecycle.self) private var lifecycle

    var body: some View {
        let state = lifecycle.updater.state

        Button("Open Ambit") {
            lifecycle.openMainWindow()
        }
        .accessibilityIdentifier("menu.open")

        Divider()

        Text(state.statusText)
            .accessibilityIdentifier("menu.updateStatus")

        if lifecycle.isWaitingForOperation {
            Text("Waiting for changes to finish installing…")
        }

        if state.canRestartToUpdate {
            Button("Restart to Update") {
                Task { await lifecycle.restartToUpdate() }
            }
            .disabled(lifecycle.leaving != nil)
            .accessibilityIdentifier("menu.restartToUpdate")
        } else if state.canCheck {
            Button(state.isFailed ? "Retry App Update" : "Check for App Updates") {
                lifecycle.updater.checkForUpdates()
            }
            .accessibilityIdentifier("menu.checkForUpdates")
        }

        Divider()

        Button("Quit Ambit") {
            NSApp.terminate(nil)
        }
        .keyboardShortcut("q")
        .accessibilityIdentifier("menu.quit")
    }
}

/// The status item's icon. It is always rendered, so it registers how to open the main window.
struct MenuBarLabel: View {
    @Environment(AppLifecycle.self) private var lifecycle
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Image(systemName: lifecycle.updater.state.canRestartToUpdate ? "square.stack.3d.up.fill" : "square.stack.3d.up")
            .accessibilityLabel("Ambit")
            .onAppear {
                lifecycle.openMainWindowAction = { openWindow(id: AmbitApp.mainWindowID) }
            }
    }
}
