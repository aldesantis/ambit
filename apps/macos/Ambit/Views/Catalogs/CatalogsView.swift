import SwiftUI

struct CatalogsView: View {
    let setup: SetupModel

    var body: some View {
        ScrollView { CatalogUpdatesView(setup: setup) }
    }
}
