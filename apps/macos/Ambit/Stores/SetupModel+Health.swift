import Foundation

extension SetupModel {
    /// Installation and health of the saved config in one value, for the header badge. `nil`
    /// until the status has been read (`health.refreshStatus()`), and while the setup has no
    /// saved config. Draft and operation states belong to `badge`, which this does not replace.
    var healthSummary: SetupHealthSummary? { health.summary }
}
