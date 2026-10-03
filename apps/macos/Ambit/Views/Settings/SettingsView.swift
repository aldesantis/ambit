import SwiftUI

/// The Settings window. Each pane lives in its own file in this folder.
struct SettingsView: View {
    var body: some View {
        TabView {
            Tab("General", systemImage: "gearshape") {
                GeneralSettingsView()
            }
        }
        .frame(width: 460)
    }
}

struct GeneralSettingsView: View {
    var body: some View {
        Form {
            LabeledContent("Version", value: Bundle.main.versionDescription)
        }
        .formStyle(.grouped)
    }
}

extension Bundle {
    /// "0.5.0 (1)".
    var versionDescription: String {
        let version = object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        let build = object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "?"
        return "\(version) (\(build))"
    }
}
