import SwiftUI

struct AgentToolsView: View {
    let setup: SetupModel

    var body: some View {
        ContentUnavailableView(
            "Agent Tools", systemImage: "hammer",
            description: Text("The agent tools this setup installs capabilities for."))
    }
}
