import SwiftUI

struct UpdatesSettingsView: View {
    @Environment(AppLifecycle.self) private var lifecycle

    var body: some View {
        let updater = lifecycle.updater
        let state = updater.state

        Form {
            Section {
                LabeledContent("Current version", value: Bundle.main.versionDescription)

                VStack(alignment: .leading, spacing: 6) {
                    Text(state.statusText)
                        .accessibilityIdentifier("settings.updateStatus")
                    if state.isBusy {
                        if let progress = state.progress {
                            ProgressView(value: progress)
                        } else {
                            ProgressView()
                                .progressViewStyle(.linear)
                        }
                    }
                    if lifecycle.isWaitingForOperation {
                        Text("Waiting for changes to finish installing…")
                            .foregroundStyle(.secondary)
                    }
                }

                if let checked = updater.lastCheckedAt {
                    LabeledContent("Last checked") {
                        Text(checked, format: .relative(presentation: .named))
                    }
                }

                HStack {
                    Button(state.isFailed ? "Retry" : "Check for App Updates") {
                        updater.checkForUpdates()
                    }
                    .disabled(!state.canCheck)
                    .accessibilityIdentifier("settings.checkForUpdates")

                    if state.canRestartToUpdate {
                        Button("Restart to Update") {
                            Task { await lifecycle.restartToUpdate() }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(lifecycle.leaving != nil)
                        .accessibilityIdentifier("settings.restartToUpdate")
                    }
                }
            } footer: {
                Text(
                    "Ambit checks for app updates at launch and every six hours and downloads them automatically. A downloaded update installs when you quit Ambit. Updating Ambit never changes your catalogs or setups."
                )
                .foregroundStyle(.secondary)
            }
        }
        .formStyle(.grouped)
    }
}
