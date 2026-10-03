import Foundation

extension SetupModel {
    /// The Health area of this setup, created on first use and kept for the setup's lifetime.
    var health: HealthModel {
        SetupAreaModels.model(for: self, area: "health") { HealthModel(setup: $0) }
    }

    /// Installation and health of the saved config in one value, for the header badge. `nil`
    /// until the status has been read (`health.refreshStatus()`), and while the setup has no
    /// saved config. Draft and operation states belong to `badge`, which this does not replace.
    var healthSummary: SetupHealthSummary? { health.summary }
}
