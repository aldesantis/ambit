// State of one setup root. Each feature area extends this type in its own
// `SetupModel+<Area>.swift` file (for example `SetupModel+Catalogs.swift`) and keeps its stored
// state in a separate area model that this type owns, so areas never edit each other's files.

import Foundation
import Observation

@MainActor
@Observable
final class SetupModel {
    /// The status shown next to a setup's name.
    enum Badge: Equatable {
        case unconfigured
        case pendingChanges
        case installing
        case installed
        case notFullyInstalled
        case folderUnavailable
        case error
    }

    let id: SetupID
    let root: URL
    @ObservationIgnored let engine: any EngineService
    @ObservationIgnored let session: any SetupSessionService

    private(set) var snapshot: SetupSnapshot?
    private(set) var isLoading = false

    init(id: SetupID, root: URL, engine: any EngineService) {
        self.id = id
        self.root = root
        self.engine = engine
        session = engine.openSetup(root: root.path)
    }

    var displayName: String {
        switch id {
        case .personal: String(localized: "Personal setup")
        case .project: root.lastPathComponent
        }
    }

    /// True while the draft differs from the saved config.
    var hasPendingChanges: Bool { false }

    var badge: Badge? {
        guard let snapshot else {
            return nil
        }

        switch snapshot.config {
        case .missing: return hasPendingChanges ? .pendingChanges : .unconfigured
        case .ambiguous, .invalid: return .error
        case .valid: return hasPendingChanges ? .pendingChanges : .installed
        }
    }

    /// Re-reads the config. Never fetches catalogs.
    func refresh() async {
        isLoading = true
        defer { isLoading = false }
        snapshot = await session.snapshot()
    }
}
