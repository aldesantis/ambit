import SwiftUI

struct GeneralSettingsView: View {
    @Environment(AppLifecycle.self) private var lifecycle
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        let loginItem = lifecycle.loginItem

        Form {
            Section {
                Toggle(
                    "Launch at login",
                    isOn: Binding(get: { loginItem.isEnabled }, set: { loginItem.setEnabled($0) })
                )
                .accessibilityIdentifier("settings.launchAtLogin")

                if let notice = loginItem.notice {
                    Label(notice, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                        .accessibilityIdentifier("settings.launchAtLoginNotice")
                    if loginItem.showsSystemSettingsAction {
                        Button("Open Login Items Settings") {
                            loginItem.openSystemSettings()
                        }
                    }
                }

                if let error = loginItem.lastError {
                    Text(error)
                        .foregroundStyle(.red)
                }
            } footer: {
                Text("At login, Ambit starts in the menu bar without opening its window.")
                    .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
        .onAppear { loginItem.refresh() }
        .onChange(of: scenePhase) { _, phase in
            // The user may have changed it in System Settings meanwhile.
            if phase == .active {
                loginItem.refresh()
            }
        }
    }
}
