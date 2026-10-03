import SwiftUI

/// Confirms removing a catalog, listing the selections and rules removed with it. Cancel changes
/// nothing; Remove stages the catalog and those entries together.
struct RemoveCatalogSheet: View {
    let removal: CatalogsModel.Removal
    let catalogs: CatalogsModel

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text("Remove \(removal.catalog.name)?")
                .font(.title2.bold())

            if removal.selections.isEmpty && removal.rules.isEmpty {
                Text("Nothing in this setup selects from \(removal.catalog.name).")
            } else {
                Text("These entries select from \(removal.catalog.name) and are removed with it:")
                List {
                    if !removal.selections.isEmpty {
                        Section("Selections") {
                            ForEach(removal.selections, id: \.self) { entry in
                                Text(CatalogSelectionText.describe(entry)).font(.callout.monospaced())
                            }
                        }
                    }
                    if !removal.rules.isEmpty {
                        Section("Selection rules") {
                            ForEach(removal.rules, id: \.self) { entry in
                                Text(CatalogSelectionText.describe(entry)).font(.callout.monospaced())
                            }
                        }
                    }
                }
                .frame(minHeight: 80, maxHeight: 220)
                .accessibilityIdentifier("removeCatalog.references")
            }

            Text("The change is added to your pending changes. Review shows the capabilities it uninstalls before anything is applied.")
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            HStack {
                Spacer()
                Button("Cancel") { catalogs.cancelRemoval() }
                    .keyboardShortcut(.cancelAction)
                    .accessibilityIdentifier("removeCatalog.cancel")
                Button("Remove Catalog", role: .destructive) { catalogs.confirmRemoval() }
                    .keyboardShortcut(.defaultAction)
                    .accessibilityIdentifier("removeCatalog.confirm")
            }
        }
        .padding(20)
        .frame(width: 460)
    }
}
