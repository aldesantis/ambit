import Foundation

extension SetupModel {
    /// This setup's catalog update state, created on first use and kept for the life of the
    /// setup model, so a running check survives switching tabs. `store` persists check results.
    func catalogUpdates(store: AppStateStore?) -> CatalogUpdatesModel {
        CatalogUpdatesRegistry.model(for: self, store: store)
    }
}

/// Holds one `CatalogUpdatesModel` per live `SetupModel`. `SetupModel` cannot gain a stored
/// property from an extension, so the area model lives here, keyed by object identity.
@MainActor
private enum CatalogUpdatesRegistry {
    private static var models: [ObjectIdentifier: CatalogUpdatesModel] = [:]

    static func model(for setup: SetupModel, store: AppStateStore?) -> CatalogUpdatesModel {
        // A released setup's identifier can be reused, so an entry counts only while its model
        // still points at this very setup.
        models = models.filter { $0.value.setup != nil }
        let key = ObjectIdentifier(setup)
        if let model = models[key], model.setup === setup {
            return model
        }

        let model = CatalogUpdatesModel(setup: setup, store: store)
        models[key] = model
        return model
    }
}
