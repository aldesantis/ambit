import SwiftUI

struct AboutSettingsView: View {
    var body: some View {
        Form {
            Section {
                LabeledContent("Version", value: Bundle.main.versionDescription)
                    .accessibilityIdentifier("settings.version")
                if let copyright = Bundle.main.object(forInfoDictionaryKey: "NSHumanReadableCopyright") as? String {
                    LabeledContent("Copyright", value: copyright)
                }
                if let repository = URL(string: "https://github.com/aldesantis/ambit") {
                    Link("Ambit on GitHub", destination: repository)
                }
            }
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
