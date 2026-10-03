//! `ambit install`: resolve, plan every harness, authorize, write, prune, record.

use std::path::Path;
use std::sync::LazyLock;

use indexmap::IndexMap;

use crate::errors::Result;
use crate::harness::adapter::{
    AppliedArtifact, HarnessAdapter, InstallScope, PlannedArtifact, ProjectPaths, SkippedHook,
};
use crate::harness::profile::ProfileAdapter;
use crate::model::git::RefreshMode;
use crate::model::state::{ArtifactMode, State};
use crate::project::gitignore::GitignoreStatus;
use crate::project::lock::Lock;
use crate::project::prune::PrunedArtifact;
use crate::resolution::resolve::Bundle;
use crate::util::env::Env;

/// Every adapter this build ships, keyed by the name `harnesses` uses.
pub static ADAPTERS: LazyLock<IndexMap<&'static str, ProfileAdapter>> =
    LazyLock::new(|| todo!("port project/install.ts:ADAPTERS"));

/// How an install was asked to behave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstallOptions {
    /// Fail rather than write when resolution would change the lock.
    pub frozen: bool,
    /// Resolve from the catalog cache alone, failing rather than fetching.
    pub offline: bool,
    /// Take ownership of existing unowned targets instead of refusing them.
    pub adopt: bool,
    /// `--copy` / `--link`: materialize every skill this way, whatever its source would have
    /// chosen. Absent means each skill follows its source.
    pub mode: Option<ArtifactMode>,
}

/// One adapter and the artifacts it would write.
#[derive(Clone)]
pub struct AdapterPlan {
    pub adapter: &'static dyn HarnessAdapter,
    pub plan: Vec<PlannedArtifact>,
}

/// A project resolved and planned, with nothing written yet.
///
/// Every mutating command starts from this: `install` applies it, `--dry-run` prints it, and
/// `prune` uses it to know what the current bundle keeps. Sharing it keeps the three from
/// disagreeing about what the bundle is.
#[derive(Clone)]
pub struct PlannedInstall {
    pub bundle: Bundle,
    /// The harnesses planned for, deduplicated and sorted.
    pub harnesses: Vec<String>,
    /// Each adapter and its own plan, in harness order.
    pub plans: Vec<AdapterPlan>,
    /// Every adapter's plan flattened: what ownership and pruning are answered against.
    pub artifacts: Vec<PlannedArtifact>,
    /// Hooks a configured harness cannot express, in harness order. Reported, never fatal.
    pub skipped: Vec<SkippedHook>,
    /// What the last install recorded owning.
    pub prior: State,
    pub lock: Lock,
    /// The lock as the bytes an install would write, which is what `--frozen` compares.
    pub lock_text: String,
}

/// What `install --dry-run` reports: everything the run would do, with the project untouched.
#[derive(Clone, Debug)]
pub struct InstallPreview {
    pub bundle: Bundle,
    pub harnesses: Vec<String>,
    /// What install would write.
    pub artifacts: Vec<PlannedArtifact>,
    /// What install would skip: a hook a configured harness cannot express.
    pub skipped: Vec<SkippedHook>,
    /// What install would remove, from state alone.
    pub pruned: Vec<PrunedArtifact>,
    pub lock: Lock,
    /// Whether `ambit.lock` would change.
    pub lock_changed: bool,
    /// Whether each managed `.gitignore` block would change, one row per file.
    pub gitignore: Vec<GitignoreStatus>,
}

/// What an install did, for the command to report.
#[derive(Clone, Debug)]
pub struct InstallResult {
    pub bundle: Bundle,
    /// The harnesses written for, deduplicated and sorted.
    pub harnesses: Vec<String>,
    /// Everything now owned, in the order the adapters wrote it.
    pub artifacts: Vec<AppliedArtifact>,
    /// Hooks a configured harness could not express, and so was not given.
    pub skipped: Vec<SkippedHook>,
    /// What the previous install owned and this one does not, removed by path.
    pub pruned: Vec<PrunedArtifact>,
    /// What was written to `ambit.lock`.
    pub lock: Lock,
}

/// Which config the harnesses will read this install as: `User` at the home directory, `Project`
/// anywhere else.
///
/// Detected from the root rather than declared in `ambit.yml`: a user-level install that did not
/// say so writes hooks resolving into whatever project is open, which fails silently and is
/// exploitable, so there is nothing here to opt into.
///
/// `HOME` is honoured over the platform's own answer, as `cache_root` (`model/git.rs`) does, so a
/// test can point a home directory somewhere disposable.
pub fn install_scope(root: &Path, env: &Env) -> InstallScope {
    let _ = (root, env);
    todo!("port project/install.ts:installScope")
}

/// Resolves configured harness names to adapters.
///
/// Shared with `status.rs`, which has to plan through exactly the adapters install would use.
///
/// # Errors
///
/// Exit 2 for a harness this build has no adapter for: silently skipping it would leave a project
/// believing it was installed.
pub fn adapters_for(harnesses: &[String]) -> Result<Vec<&'static dyn HarnessAdapter>> {
    let _ = harnesses;
    todo!("port project/install.ts:adaptersFor")
}

/// Every adapter's plan, with each artifact planned exactly once.
///
/// The skills directory is shared: every harness plans the same `.agents/skills/<name>` targets,
/// and two harnesses of one family plan the same skills link. A path is an artifact's identity, so
/// the first adapter to name one plans it and the rest defer. A config file two harnesses write the
/// *same* entries into is deduped the same way; anything else differing about a config artifact
/// makes it a second write.
///
/// `adapters` is the harnesses to plan for, sorted so the result does not depend on `ambit.yml`'s
/// spelling. This order decides who plans a shared target.
pub fn plan_for(
    adapters: &[&'static dyn HarnessAdapter],
    bundle: &Bundle,
    project: &ProjectPaths,
) -> Vec<AdapterPlan> {
    let _ = (adapters, bundle, project);
    todo!("port project/install.ts:planFor")
}

/// What the command doing the planning contributes, as against what the CLI parsed into
/// [`InstallOptions`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanContext {
    /// How a catalog with no pin to reproduce may consult its remote. Absent means not at all.
    pub refresh: Option<RefreshMode>,
    /// Catalogs whose recorded pin this run is deliberately moving past, by name.
    ///
    /// `ambit update`'s, and only `ambit update`'s. It has already advanced the shared clone's refs
    /// to the commits it just reported, and the lock on disk still holds the commits it is
    /// replacing, so honouring those pins would make the install undo the update it is part of.
    pub released: Vec<String>,
}

/// Resolves the project and plans every adapter's writes, touching nothing.
///
/// `project_dir` is the project root, absolute; `env` is the command's environment (the cache
/// location, and `HOME` for [`install_scope`]).
///
/// # Errors
///
/// Exit 2 for a malformed config or catalog, an unknown harness, an unreadable state file, an
/// unreadable `ambit.lock`, or a locked commit the repository does not have; exit 3 for a
/// resolution error; exit 4 if a fetch fails, or under `--offline` when the cache cannot answer.
pub fn plan_install(
    project_dir: &Path,
    env: &Env,
    options: InstallOptions,
    plan: &PlanContext,
) -> Result<PlannedInstall> {
    let _ = (project_dir, env, options, plan);
    todo!("port project/install.ts:planInstall")
}

/// What an install would do, without doing any of it: `install --dry-run`.
///
/// Ownership is checked, because a refusal is part of what would happen.
///
/// # Errors
///
/// Everything [`install_project`] returns before its first write: exit 2 for a malformed config or
/// an unowned target, exit 3 for a resolution error, exit 4 for a fetch, exit 5 under `--frozen`.
pub fn preview_install(
    project_dir: &Path,
    env: &Env,
    options: InstallOptions,
) -> Result<InstallPreview> {
    let _ = (project_dir, env, options);
    todo!("port project/install.ts:previewInstall")
}

/// Resolves the project and materializes the bundle.
///
/// `released` is the catalogs whose recorded pin this install is moving past (`ambit update`'s;
/// see [`PlanContext::released`]). Empty for every other caller.
///
/// # Errors
///
/// Exit 2 for a malformed config or catalog, an unknown harness, a target path or config key ambit
/// does not own and was not told to adopt, or a locked commit the repository does not have; exit 4
/// if a fetch fails, or under `--offline` when the cache cannot answer; exit 5 under `--frozen` when
/// the committed lock is not what resolution produces.
pub fn install_project(
    project_dir: &Path,
    env: &Env,
    options: InstallOptions,
    released: &[String],
) -> Result<InstallResult> {
    let _ = (project_dir, env, options, released);
    todo!("port project/install.ts:installProject")
}
