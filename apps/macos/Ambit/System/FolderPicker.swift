import AppKit

@MainActor
protocol FolderPicker {
    /// Asks the user for one folder. `nil` means the user canceled.
    func pickFolder(title: String, prompt: String, startingAt: URL?) async -> URL?
}

@MainActor
struct OpenPanelFolderPicker: FolderPicker {
    func pickFolder(title: String, prompt: String, startingAt: URL?) async -> URL? {
        let panel = NSOpenPanel()
        panel.title = title
        panel.prompt = prompt
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.directoryURL = startingAt

        let response: NSApplication.ModalResponse
        if let window = NSApp.keyWindow {
            response = await panel.beginSheetModal(for: window)
        } else {
            response = await panel.begin()
        }
        return response == .OK ? panel.url : nil
    }
}

/// Returns a fixed folder without showing UI. Used for `AMBIT_TEST_PICK_FOLDER` and unit tests.
@MainActor
struct FixedFolderPicker: FolderPicker {
    var folder: URL?

    func pickFolder(title: String, prompt: String, startingAt: URL?) async -> URL? {
        folder
    }
}
