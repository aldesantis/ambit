import Foundation

extension SetupModel {
    /// This setup's catalog update state, created on first use and kept for the life of the
    /// setup model, so a running check survives switching tabs. `store` persists check results.
    func catalogUpdates(store: AppStateStore?) -> CatalogUpdatesModel {
        if let updatesModel {
            return updatesModel
        }

        let model = CatalogUpdatesModel(setup: self, store: store)
        updatesModel = model
        return model
    }
}
