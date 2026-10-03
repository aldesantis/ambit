// The complete environment the engine runs with. The Rust library never reads the process
// environment, so everything it, and the git it starts, may need is decided here: a home, a
// minimal PATH, the XDG roots, the ssh agent, and the git executable to use.

import Foundation

enum LiveEngineEnvironment {
    /// Names the git executable the engine runs (`ambit_core::model::git::GIT_PROGRAM_VAR`).
    static let gitProgramVariable = "AMBIT_GIT_PROGRAM"

    /// Directories every macOS has. The bundled git, when present, goes first.
    static let systemPath = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]

    /// Copied from the process when set, so the app shares the CLI's cache and git's config, and
    /// ssh remotes can use the user's agent.
    static let forwarded = [
        "XDG_CACHE_HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME",
        "SSH_AUTH_SOCK", "TMPDIR", "USER", "LOGNAME", "LANG",
    ]

    /// The git copied into the app by scripts/embed-git.sh, when this build has one.
    static func bundledGit(in bundle: Bundle = .main) -> URL? {
        guard let resources = bundle.resourceURL else {
            return nil
        }
        let git = resources.appending(path: "git/bin/git", directoryHint: .notDirectory)
        return FileManager.default.isExecutableFile(atPath: git.path) ? git : nil
    }

    /// - Parameters:
    ///   - home: The engine's `HOME`: the user's home, or the test home under UI testing.
    ///   - process: The variables to forward from (the process environment).
    ///   - forwardProcess: False under UI testing, so a developer's XDG roots or agent never leak
    ///     into a test, and every location derives from `home`.
    ///   - git: The git executable to run, or `nil` for `git` on `PATH`.
    static func make(
        home: URL, process: [String: String] = ProcessInfo.processInfo.environment, forwardProcess: Bool = true,
        git: URL? = bundledGit()
    ) -> [String: String] {
        var env: [String: String] = [:]

        if forwardProcess {
            for name in forwarded {
                if let value = process[name], !value.isEmpty {
                    env[name] = value
                }
            }
        }

        env["HOME"] = home.path
        var path = systemPath
        if let git {
            path.insert(git.deletingLastPathComponent().path, at: 0)
            env[gitProgramVariable] = git.path
        }
        env["PATH"] = path.joined(separator: ":")
        return env
    }
}
