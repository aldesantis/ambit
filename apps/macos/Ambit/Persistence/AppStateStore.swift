// The app's own state: `state.json` in `~/Library/Application Support/com.nebulab.ambit/`.
// Setup files are never stored here; they stay in each setup root.
//
// Decoding is tolerant: every field is optional in the file, so a state written by an older or
// newer version loads with defaults instead of failing. A file that is not valid JSON is moved
// aside to `state.json.corrupt` and the app starts from defaults rather than refusing to launch.

import Foundation
import OSLog

/// A setup the sidebar can select. Project paths are canonical (see `ProjectRegistry`).
enum SetupID: Hashable, Codable, Sendable {
    case personal
    case project(path: String)
}

struct RememberedProject: Codable, Hashable, Sendable, Identifiable {
    var path: String
    var addedAt: Date

    var id: String { path }
    var name: String { URL(fileURLWithPath: path).lastPathComponent }
}

struct AppState: Codable, Equatable, Sendable {
    static let currentVersion = 1

    var version = currentVersion
    /// In the order they were added.
    var projects: [RememberedProject] = []
    var lastActiveSetup: SetupID?
    /// True once the first launch registered the login item, so a later opt-out sticks.
    var launchAtLoginInitialized = false
    /// The user's launch-at-login choice; on until they turn it off.
    var launchAtLoginPreferred = true

    init() {}

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        version = try container.decodeIfPresent(Int.self, forKey: .version) ?? Self.currentVersion
        projects = try container.decodeIfPresent([RememberedProject].self, forKey: .projects) ?? []
        lastActiveSetup = try? container.decodeIfPresent(SetupID.self, forKey: .lastActiveSetup)
        launchAtLoginInitialized = try container.decodeIfPresent(Bool.self, forKey: .launchAtLoginInitialized) ?? false
        launchAtLoginPreferred = try container.decodeIfPresent(Bool.self, forKey: .launchAtLoginPreferred) ?? true
    }
}

@MainActor
final class AppStateStore {
    static let fileName = "state.json"

    let fileURL: URL
    private(set) var state: AppState

    /// The live location: `~/Library/Application Support/com.nebulab.ambit`.
    static var defaultDirectory: URL {
        URL.applicationSupportDirectory.appending(path: "com.nebulab.ambit", directoryHint: .isDirectory)
    }

    /// Loads the state in `directory`, which is created on the first save.
    init(directory: URL) {
        fileURL = directory.appending(path: Self.fileName, directoryHint: .notDirectory)
        state = Self.load(from: fileURL)
    }

    /// Applies `change` and writes the file atomically. The in-memory state changes only when
    /// the write succeeds.
    func update(_ change: (inout AppState) -> Void) throws {
        var next = state
        change(&next)
        next.version = AppState.currentVersion
        guard next != state else {
            return
        }

        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        encoder.dateEncodingStrategy = .iso8601
        let data = try encoder.encode(next)
        try FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(), withIntermediateDirectories: true)
        try data.write(to: fileURL, options: .atomic)
        // Round-trip so memory matches what a relaunch reads (dates lose sub-second precision).
        state = (try? Self.decode(data)) ?? next
    }

    private static func decode(_ data: Data) throws -> AppState {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try decoder.decode(AppState.self, from: data)
    }

    private static func load(from url: URL) -> AppState {
        let data: Data
        do {
            data = try Data(contentsOf: url)
        } catch {
            return AppState()
        }

        do {
            return try decode(data)
        } catch {
            Logger.persistence.error("Unreadable \(url.path, privacy: .public): \(error, privacy: .public)")
            let aside = url.appendingPathExtension("corrupt")
            try? FileManager.default.removeItem(at: aside)
            try? FileManager.default.moveItem(at: url, to: aside)
            return AppState()
        }
    }
}

extension Logger {
    static let persistence = Logger(subsystem: "com.nebulab.ambit", category: "persistence")
}
