import SwiftUI

struct CapabilitiesView: View {
    let setup: SetupModel

    var body: some View {
        ContentUnavailableView(
            "Capabilities", systemImage: "puzzlepiece.extension",
            description: Text("Skills, packs, MCP servers and hooks from this setup's catalogs."))
    }
}
