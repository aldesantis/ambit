//! `.ambit/state.json`: what ambit owns in a project.

use std::path::{Path, PathBuf};

use indexmap::IndexSet;

use crate::errors::Result;
use crate::model::documents::{DocumentFormat, DocumentShape};
use crate::util::string_enum;

/// The machine-local directory ambit keeps its state in. Always gitignored.
pub const STATE_DIRNAME: &str = ".ambit";

/// The state file within it.
pub const STATE_FILENAME: &str = "state.json";

/// The only state version this build understands.
pub const STATE_VERSION: i64 = 1;

string_enum! {
    /// What an owned artifact is. `HarnessConfig` carries `managed_keys` instead of a `mode`.
    pub enum ArtifactKind {
        HarnessConfig => "harness-config",
        HookDir => "hook-dir",
        SkillDir => "skill-dir",
        SkillsLink => "skills-link",
    }
}

/// Every artifact kind, in declaration order.
pub const ARTIFACT_KINDS: &[ArtifactKind] = ArtifactKind::ALL;

string_enum! {
    /// How a materialized directory's source reaches its target: copied for remote sources,
    /// symlinked for local ones.
    pub enum ArtifactMode {
        Copy => "copy",
        Link => "link",
    }
}

/// Every artifact mode, in declaration order.
pub const ARTIFACT_MODES: &[ArtifactMode] = ArtifactMode::ALL;

/// One file or directory ambit created, addressed the only way that survives a move: relatively.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedArtifact {
    /// Project-relative, `/`-separated.
    pub path: String,
    pub kind: ArtifactKind,
    /// Set for `skill-dir`, `hook-dir` and `skills-link`.
    pub mode: Option<ArtifactMode>,
    /// Set for `harness-config`: the dotted keys within the file ambit owns.
    pub managed_keys: Option<Vec<String>>,
    /// Set for `harness-config`: how the file is parsed and written.
    ///
    /// Recorded because `prune` and `clean` act from state alone: they must edit a
    /// `.codex/config.toml` as TOML without re-resolving the project. Absent reads as `json`, which
    /// is what every artifact written before this field existed was.
    pub format: Option<DocumentFormat>,
    /// Set for `harness-config`: how the managed section is laid out.
    ///
    /// Not derivable from `format`: `.mcp.json` and `.claude/settings.json` are both JSON, and the
    /// second holds one array per event rather than a table keyed by name. Absent reads as `map`.
    pub shape: Option<DocumentShape>,
}

/// The contents of `.ambit/state.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub version: i64,
    /// The harnesses the artifacts were written for.
    pub harnesses: Vec<String>,
    pub artifacts: Vec<OwnedArtifact>,
}

impl State {
    /// What a project with no state file is treated as: ambit owns nothing there yet.
    pub fn empty() -> Self {
        Self {
            version: STATE_VERSION,
            harnesses: Vec::new(),
            artifacts: Vec::new(),
        }
    }
}

/// Where the state file lives for a project.
pub fn state_file_path(project_dir: &Path) -> PathBuf {
    let _ = project_dir;
    todo!("port model/state.ts:stateFilePath")
}

/// The set of paths ambit may delete or overwrite.
pub fn owned_paths(state: &State) -> IndexSet<String> {
    let _ = state;
    todo!("port model/state.ts:ownedPaths")
}

/// Renders state as the bytes written to disk: keys sorted, artifacts by path, trailing newline.
pub fn serialize_state(state: &State) -> String {
    let _ = state;
    todo!("port model/state.ts:serializeState")
}

/// Parses a state document. `file` is how it is named in error messages, conventionally
/// project-relative.
///
/// # Errors
///
/// Exit 2 for malformed JSON, an unsupported version, or a bad artifact entry: an unreadable
/// ownership record is exactly when ambit must stop rather than guess.
pub fn parse_state(text: &str, file: &str) -> Result<State> {
    let _ = (text, file);
    todo!("port model/state.ts:parseState")
}

/// Reads a project's state, treating an absent file as "ambit owns nothing here".
///
/// # Errors
///
/// Exit 2 if the file exists but cannot be trusted.
pub fn read_state(project_dir: &Path) -> Result<State> {
    let _ = project_dir;
    todo!("port model/state.ts:readState")
}

/// Writes a project's state.
///
/// Called only after the filesystem changes it describes have succeeded, so a crash leaves
/// artifacts owned and recoverable rather than orphaned.
///
/// # Errors
///
/// Exit 2 when the file cannot be written.
pub fn write_state(project_dir: &Path, state: &State) -> Result<()> {
    let _ = (project_dir, state);
    todo!("port model/state.ts:writeState")
}
