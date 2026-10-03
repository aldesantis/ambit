import SwiftUI

struct HealthView: View {
    let setup: SetupModel

    var body: some View {
        ContentUnavailableView(
            "Health", systemImage: "stethoscope",
            description: Text("Whether installed capabilities match this setup."))
    }
}
