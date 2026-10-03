import SwiftUI

/// Adds a catalog or changes one's source, revision or name. A new or changed source must load
/// (Verify) before the change can be staged.
struct CatalogEditorSheet: View {
    @Bindable var editor: CatalogEditorModel
    let catalogs: CatalogsModel
    let folderPicker: any FolderPicker
    @Environment(AppModel.self) private var app

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(editor.isAdding ? "Add Catalog" : "Edit \(editor.originalName ?? "")")
                .font(.title2.bold())

            Form {
                Section {
                    HStack {
                        TextField("Source", text: $editor.source, prompt: Text("owner/repo, a Git URL, or a folder"))
                            .accessibilityIdentifier("catalogEditor.source")
                        Button("Choose Folder…") { chooseFolder() }
                            .accessibilityIdentifier("catalogEditor.chooseFolder")
                    }
                    sourceFootnote
                }

                Section {
                    TextField("Name", text: $editor.name)
                        .accessibilityIdentifier("catalogEditor.name")
                    if let error = editor.nameError, !editor.trimmedSource.isEmpty || !editor.trimmedName.isEmpty {
                        Text(error.message)
                            .font(.caption)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("catalogEditor.nameError")
                    } else if editor.isRenaming {
                        Text("Selections from this catalog are renamed with it.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else {
                        Text("Selections refer to items as name/item.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }

                if !editor.isLocal {
                    Section {
                        DisclosureGroup("Advanced", isExpanded: $editor.showsRevision) {
                            TextField("Revision", text: $editor.revision, prompt: Text("Default branch"))
                                .accessibilityIdentifier("catalogEditor.revision")
                            Text("A branch, tag or commit. Leave empty to follow the repository's default branch.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
            .formStyle(.grouped)

            verificationView

            if let error = editor.saveError {
                Label(error.message, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.red)
            }

            HStack {
                if editor.sourceChanged {
                    if editor.isVerifying {
                        Button("Stop Verifying") { editor.cancelVerification() }
                            .disabled(app.operations.current?.isCancellable == false)
                            .accessibilityIdentifier("catalogEditor.stop")
                    } else {
                        Button("Verify") { editor.verify() }
                            .disabled(!editor.canVerify)
                            .accessibilityIdentifier("catalogEditor.verify")
                    }
                }
                Spacer()
                Button("Cancel") {
                    Task { await catalogs.cancelEditor() }
                }
                .keyboardShortcut(.cancelAction)
                .accessibilityIdentifier("catalogEditor.cancel")
                Button(editor.isAdding ? "Add Catalog" : "Save Changes") { catalogs.saveEditor() }
                    .keyboardShortcut(.defaultAction)
                    .buttonStyle(.borderedProminent)
                    .disabled(!editor.canSave)
                    .accessibilityIdentifier("catalogEditor.save")
            }
        }
        .padding(20)
        .frame(width: 520)
        .interactiveDismissDisabled(editor.isVerifying)
    }

    @ViewBuilder private var sourceFootnote: some View {
        if let error = editor.sourceError {
            Text(error.message)
                .font(.caption)
                .foregroundStyle(.red)
                .accessibilityIdentifier("catalogEditor.sourceError")
        } else if let kind = editor.sourceKind {
            HStack(spacing: 6) {
                CatalogKindLabel(kind: kind)
                if let path = editor.resolvedLocalPath {
                    Text(path)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
        } else {
            Text("GitHub shorthand (owner/repo), an HTTPS or SSH Git URL, or a local folder. Relative paths start at the setup folder.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    @ViewBuilder private var verificationView: some View {
        switch editor.verification {
        case .idle:
            if editor.sourceChanged {
                Text("Verify loads the catalog before it is added to your changes.")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
        case .verifying:
            CatalogFetchProgressView(operation: app.operations.current) { editor.cancelVerification() }
                .accessibilityIdentifier("catalogEditor.verifying")
        case let .verified(probe):
            VStack(alignment: .leading, spacing: 6) {
                Label(verifiedText(probe), systemImage: "checkmark.circle.fill")
                    .foregroundStyle(.green)
                    .accessibilityIdentifier("catalogEditor.verified")
                if !probe.unmatched.isEmpty {
                    UnmatchedEntriesView(entries: probe.unmatched, remove: nil)
                }
            }
        case let .failed(error):
            CatalogProblemView(
                title: String(localized: "The catalog could not be loaded"), error: error, account: app.account,
                retry: { editor.verify() })
                .accessibilityIdentifier("catalogEditor.failed")
        }
    }

    private func verifiedText(_ probe: CatalogEditorModel.Probe) -> String {
        let revision =
            if probe.local {
                String(localized: "Loaded local files")
            } else if let commit = probe.commit {
                String(localized: "Loaded commit \(CatalogText.short(commit))")
            } else {
                String(localized: "Loaded")
            }
        guard let counts = probe.counts else {
            return revision
        }
        let items = String(
            localized:
                "^[\(counts.skills) skill](inflect: true), ^[\(counts.packs) pack](inflect: true), ^[\(counts.mcps) MCP server](inflect: true), ^[\(counts.hooks) hook](inflect: true)"
        )
        return "\(revision): \(items)"
    }

    private func chooseFolder() {
        Task {
            let start = editor.resolvedLocalPath.map { URL(fileURLWithPath: $0) } ?? editor.setup.root
            if let folder = await folderPicker.pickFolder(
                title: String(localized: "Choose Catalog Folder"), prompt: String(localized: "Choose"),
                startingAt: start)
            {
                editor.useFolder(folder)
            }
        }
    }
}
