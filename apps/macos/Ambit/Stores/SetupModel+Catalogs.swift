// The Catalogs area's hooks into `SetupModel`.

import Foundation

extension SetupModel {
    /// The config text the next edit applies to: the draft's, otherwise the saved file's. `nil`
    /// when the setup has no usable config.
    var currentConfigText: String? {
        if let draft {
            return draft.text
        }
        if case let .valid(_, _, text, _) = snapshot?.config {
            return text
        }
        return nil
    }

    /// The catalogs of the saved config, to tell which rows the draft added or changed.
    var savedCatalogs: [CatalogEntry] {
        if case let .valid(_, _, _, summary) = snapshot?.config {
            return summary.catalogs
        }
        return []
    }

    /// The absolute path a local catalog source names. Relative paths resolve against the root.
    func resolvedLocalPath(_ source: String) -> String {
        var path = source
        if path.hasPrefix("path:") {
            path.removeFirst("path:".count)
        }
        if path.hasPrefix("/") {
            return URL(fileURLWithPath: path).standardized.path
        }
        return root.appending(path: path).standardized.path
    }

    /// The source to save for a folder the user picked: relative to a project's root when the
    /// folder is inside it, so the config keeps working wherever the project is checked out;
    /// absolute otherwise, and always for the Personal setup.
    func catalogSource(forFolder folder: URL) -> String {
        let folderPath = folder.standardizedFileURL.resolvingSymlinksInPath().path
        let rootPath = root.standardizedFileURL.resolvingSymlinksInPath().path

        guard case .project = id else {
            return folderPath
        }
        if folderPath == rootPath {
            return "."
        }
        if folderPath.hasPrefix(rootPath + "/") {
            return "./" + folderPath.dropFirst(rootPath.count + 1)
        }
        return folderPath
    }
}
