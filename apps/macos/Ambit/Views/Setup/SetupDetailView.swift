import SwiftUI

/// The areas of a setup. Each area's view lives in `Views/<Area>/` and is owned by that area.
enum SetupSection: String, CaseIterable, Identifiable {
    case capabilities, catalogs, agentTools, health

    var id: Self { self }
}

struct SetupDetailView: View {
    let setup: SetupModel
    @State private var section: SetupSection = .capabilities

    var body: some View {
        VStack(spacing: 0) {
            SetupHeader(setup: setup)
                .padding(.horizontal, 20)
                .padding(.vertical, 16)

            TabView(selection: $section) {
                Tab("Capabilities", systemImage: "puzzlepiece.extension", value: .capabilities) {
                    CapabilitiesView(setup: setup)
                }
                Tab("Catalogs", systemImage: "books.vertical", value: .catalogs) {
                    CatalogsView(setup: setup)
                }
                Tab("Agent Tools", systemImage: "hammer", value: .agentTools) {
                    AgentToolsView(setup: setup)
                }
                Tab("Health", systemImage: "stethoscope", value: .health) {
                    HealthView(setup: setup)
                }
            }
            .padding([.horizontal, .bottom], 20)
        }
        .navigationTitle(setup.displayName)
        .toolbar {
            ToolbarItem {
                Button {
                    Task { await setup.refresh() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                }
                .keyboardShortcut("r")
                .disabled(setup.isLoading)
                .help("Read the setup again (⌘R)")
            }
        }
        .task {
            await setup.refresh()
        }
    }
}
