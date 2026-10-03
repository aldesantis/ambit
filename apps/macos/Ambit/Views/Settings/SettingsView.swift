import SwiftUI

/// The Settings window. Each pane lives in its own file in this folder.
struct SettingsView: View {
    var body: some View {
        TabView {
            Tab("General", systemImage: "gearshape") {
                GeneralSettingsView()
            }
            Tab("Account", systemImage: "person.crop.circle") {
                AccountSettingsView()
            }
            Tab("Updates", systemImage: "arrow.down.circle") {
                UpdatesSettingsView()
            }
            Tab("About", systemImage: "info.circle") {
                AboutSettingsView()
            }
        }
        .frame(width: 460)
    }
}
