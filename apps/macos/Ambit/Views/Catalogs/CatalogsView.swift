import SwiftUI

struct CatalogsView: View {
    let setup: SetupModel

    var body: some View {
        ContentUnavailableView(
            "Catalogs", systemImage: "books.vertical",
            description: Text("The local folders and Git repositories this setup takes capabilities from."))
    }
}
