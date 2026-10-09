//! The harness adapter boundary.
//!
//! `plan` is pure and testable without a filesystem; `apply` is the only thing in ambit that writes
//! into a project. This split makes `--dry-run` a print of the plan rather than a second
//! implementation of installation.
//!
//! A planned artifact carries both its project-relative `path` (the identity state records, and
//! the only form that survives the project being moved) and its absolute `target`, so `apply`
//! writes exactly what `plan` decided and never re-derives a location.

use std::path::{Path, PathBuf};

use crate::errors::Result;
use crate::model::documents::{ConfigEntry, DocumentFormat, DocumentShape, JsonObject};
use crate::model::hook_entity::HookEvent;
use crate::model::state::{ArtifactKind, ArtifactMode, OwnedArtifact, State};
use crate::resolution::resolve::Bundle;
use crate::util::string_enum;

string_enum! {
    /// Which config a harness reads the installed files as.
    ///
    /// - `Project`: one project's own config, read only while that project is open.
    /// - `User`: the machine-wide config every project sees, which is what a harness reads its own
    ///   files under the home directory as.
    ///
    /// The distinction reaches the plan because a user-level file cannot name a path relative to a
    /// project: there is no one project for it to be relative to (`harness/definitions.rs`).
    pub enum InstallScope {
        Project => "project",
        User => "user",
    }
}

/// Where, and how, a bundle is being materialized.
///
/// Nothing about the environment appears here beyond `scope`: ambit writes a harness-native
/// reference for every `${VAR}` rather than a value, so a plan is a function of the bundle and the
/// project alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectPaths {
    /// Where the artifacts land, absolute: the project root, or the home directory for the
    /// user-level project. Every artifact path is relative to it.
    pub root: PathBuf,
    /// Which config the harnesses will read this install as. Absent reads as `Project`.
    ///
    /// Decided from the project directory alone, by `project_paths` (`project/install.rs`).
    pub scope: Option<InstallScope>,
    /// `--copy` / `--link`: force every skill's materialization mode for this run.
    ///
    /// Absent (the normal case) means each skill follows its own source: a pinned remote one is
    /// copied, a local one is symlinked. Carried here because `plan` must decide the mode from its
    /// arguments alone.
    pub mode: Option<ArtifactMode>,
}

/// A directory copied out of a catalog: a skill, or the bytes a hook that ships a script is made
/// of.
///
/// One type for both because they are handled identically: writing, comparing against source, and
/// reporting the landed mode are the same operation. Only the [`PlannedArtifact`] variant differs,
/// so state, pruning, `status`, and the gitignore blocks can tell a hook directory from a skill.
///
/// A hook directory is planned only for a hook whose `command` names a file its directory holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCatalogDir {
    /// Project-relative, `/`-separated.
    pub path: String,
    /// Absolute target.
    pub target: PathBuf,
    /// Absolute source directory within the catalog.
    pub source: PathBuf,
    /// How the source reaches the target: copied for a source pinned to a commit, symlinked for a
    /// local directory someone edits. `--copy`/`--link` force one for the whole run. A copy keeps
    /// file modes, so a script's executable bit survives either way.
    pub mode: ArtifactMode,
    /// The skill's or hook's name, so a failure names it and not only a path.
    pub name: String,
}

/// A skill directory to materialize.
pub type PlannedSkillDir = PlannedCatalogDir;

/// A hook's own directory to materialize.
pub type PlannedHookDir = PlannedCatalogDir;

/// A directory symlink pointing a harness at the shared skills directory.
///
/// Separate from a skill directory because it is one artifact regardless of what the bundle holds,
/// and it is the one thing ambit installs that a harness reads through rather than reads directly.
/// Its mode is always [`ArtifactMode::Link`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedSkillsLink {
    /// Project-relative, `/`-separated: e.g. `.claude/skills`.
    pub path: String,
    /// Absolute target.
    pub target: PathBuf,
    /// Absolute path the link points at: the shared skills directory.
    pub source: PathBuf,
}

/// A section of a harness's own config file to write.
///
/// Unlike a skill directory, the target is not ambit's to replace: the file may hold entries a
/// person added by hand, so `apply` merges into it and ownership is recorded per key.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedHarnessConfig {
    /// Project-relative, `/`-separated.
    pub path: String,
    /// Absolute target.
    pub target: PathBuf,
    /// The top-level object within the file ambit writes into.
    pub section: String,
    /// How the file is parsed and written: which driver reads and edits it.
    pub format: DocumentFormat,
    /// How the managed section is laid out: a table keyed by name, or one array per event. Absent
    /// reads as `Map`, which is what every MCP config uses.
    pub shape: Option<DocumentShape>,
    /// Root keys the file should carry beside the managed section: Cursor's `version: 1`.
    ///
    /// Seeded only where the document lacks the key. Not recorded in state, unlike `shape`:
    /// removal applies no defaults.
    pub root_defaults: Option<JsonObject>,
    /// What ambit puts there, sorted by key.
    pub entries: Vec<ConfigEntry>,
    /// `<section>.<key>` for each entry: what state records as owned.
    pub managed_keys: Vec<String>,
}

/// Everything an adapter can be asked to write.
///
/// `SkillDir`, `HookDir` and `SkillsLink` are owned as a whole path; `HarnessConfig` is co-owned
/// per key.
#[derive(Clone, Debug, PartialEq)]
pub enum PlannedArtifact {
    SkillDir(PlannedSkillDir),
    HookDir(PlannedHookDir),
    SkillsLink(PlannedSkillsLink),
    HarnessConfig(PlannedHarnessConfig),
}

impl PlannedArtifact {
    /// The kind state records this artifact as.
    pub fn kind(&self) -> ArtifactKind {
        match self {
            Self::SkillDir(_) => ArtifactKind::SkillDir,
            Self::HookDir(_) => ArtifactKind::HookDir,
            Self::SkillsLink(_) => ArtifactKind::SkillsLink,
            Self::HarnessConfig(_) => ArtifactKind::HarnessConfig,
        }
    }

    /// Project-relative, `/`-separated.
    pub fn path(&self) -> &str {
        match self {
            Self::SkillDir(dir) | Self::HookDir(dir) => &dir.path,
            Self::SkillsLink(link) => &link.path,
            Self::HarnessConfig(config) => &config.path,
        }
    }

    /// Absolute target.
    pub fn target(&self) -> &Path {
        match self {
            Self::SkillDir(dir) | Self::HookDir(dir) => &dir.target,
            Self::SkillsLink(link) => &link.target,
            Self::HarnessConfig(config) => &config.target,
        }
    }

    /// The materialization mode, for the kinds owned as a whole path.
    pub fn mode(&self) -> Option<ArtifactMode> {
        match self {
            Self::SkillDir(dir) | Self::HookDir(dir) => Some(dir.mode),
            Self::SkillsLink(_) => Some(ArtifactMode::Link),
            Self::HarnessConfig(_) => None,
        }
    }
}

/// A planned artifact in the shape state records it, which is all the `.gitignore` renderer reads:
/// `install --dry-run` and `doctor` both ask it about a plan that has not been applied.
impl From<&PlannedArtifact> for OwnedArtifact {
    fn from(artifact: &PlannedArtifact) -> Self {
        let config = match artifact {
            PlannedArtifact::HarnessConfig(config) => Some(config),
            _ => None,
        };

        Self {
            path: artifact.path().to_owned(),
            kind: artifact.kind(),
            mode: artifact.mode(),
            managed_keys: config.map(|config| config.managed_keys.clone()),
            format: config.map(|config| config.format),
            shape: config.and_then(|config| config.shape),
            digest: None,
        }
    }
}

/// What `apply` reports back, and what goes into state verbatim.
pub type AppliedArtifact = OwnedArtifact;

string_enum! {
    /// Why one harness cannot install one hook.
    ///
    /// - `NoMechanism`: the harness expresses hooks nowhere, which is opencode.
    /// - `NoEvent`: the harness expresses hooks, but has no spelling for this one's event.
    ///
    /// Neither is an error: the hook installs everywhere else. Failing the run would let one
    /// harness in `harnesses` veto every other harness's hooks.
    pub enum HookSkipReason {
        NoMechanism => "no-mechanism",
        NoEvent => "no-event",
    }
}

/// One hook a harness was given and cannot write, for the run to report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedHook {
    /// The harness that cannot express it.
    pub harness: String,
    /// The hook's name, as declared.
    pub hook: String,
    /// Its event, in ambit's own spelling.
    pub event: HookEvent,
    pub reason: HookSkipReason,
}

/// Code that writes a bundle into one agent tool's layout.
pub trait HarnessAdapter: Sync {
    #[cfg(test)]
    fn name(&self) -> &str;

    /// Pure: decides every path without touching disk.
    fn plan(&self, bundle: &Bundle, project: &ProjectPaths) -> Vec<PlannedArtifact>;

    /// The hooks in the bundle this harness cannot express, which `plan` leaves out.
    ///
    /// Separate from `plan` because a skipped hook is not an artifact: nothing writes it, nothing
    /// owns it, nothing prunes it. Both answers come from one predicate (`profile.rs`), so they
    /// cannot disagree about which hooks were installed.
    fn skips(&self, bundle: &Bundle) -> Vec<SkippedHook>;

    /// Writes the plan. `prior` is the state from the last install, which says what ambit may
    /// overwrite.
    ///
    /// # Errors
    ///
    /// Exit 2 for a target ambit cannot write or a config file it cannot merge into.
    fn apply(&self, plan: &[PlannedArtifact], prior: &State) -> Result<Vec<AppliedArtifact>>;
}
