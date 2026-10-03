//! `ambit prune` and `ambit clean`.

use std::path::Path;

use crate::errors::Result;
use crate::model::state::OwnedArtifact;
use crate::project::prune::PrunedArtifact;
use crate::util::env::Env;

/// How a prune was asked to behave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneOptions {
    /// Resolve from the catalog cache alone, failing rather than fetching.
    pub offline: bool,
    /// `--dry-run`: report what would be removed and touch nothing.
    pub dry_run: bool,
}

/// What a prune removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneResult {
    /// What was removed, by path; under `--dry-run`, what would be.
    pub pruned: Vec<PrunedArtifact>,
    /// What ambit still owns afterwards, which is what state now records.
    pub remaining: Vec<OwnedArtifact>,
}

/// How a clean was asked to behave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanOptions {
    /// `--dry-run`: report what would be removed and touch nothing.
    pub dry_run: bool,
}

/// What a clean removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanResult {
    /// Every owned artifact removed, by path; under `--dry-run`, what would be.
    pub removed: Vec<PrunedArtifact>,
    /// Whether `.ambit/` was there to remove.
    pub state_removed: bool,
    /// The `.gitignore` files a managed block was there to remove from.
    pub gitignore_removed: Vec<String>,
}

/// Removes the owned artifacts the current bundle no longer selects.
///
/// Ownership is not authorized first, unlike an install: nothing here writes an artifact. Writes
/// are ordered as install orders them: filesystem, then lock, then state, then the `.gitignore`
/// block. A run with nothing stale writes nothing.
///
/// # Errors
///
/// Exit 2 for a malformed config or catalog, an unknown harness, an unreadable state file, a
/// co-owned config file that cannot be parsed, or an ambiguous `.gitignore` block; exit 3 for a
/// resolution error; exit 4 if a fetch fails, or under `--offline` when the cache cannot answer.
pub fn prune_project(project_dir: &Path, env: &Env, options: PruneOptions) -> Result<PruneResult> {
    let _ = (project_dir, env, options);
    todo!("port project/clean.ts:pruneProject")
}

/// Removes everything ambit owns in a project.
///
/// Order: artifacts, then the `.gitignore` block, then `.ambit/`. State is removed last, so the
/// whole command stays retryable once an ambiguous `.gitignore` is fixed.
///
/// # Errors
///
/// Exit 2 for an unreadable state file, a co-owned config file that cannot be parsed, a managed key
/// state records in a form this build cannot act on, or a `.gitignore` whose markers are ambiguous.
/// No catalog is read and nothing is resolved, so there is no exit 3 or 4.
pub fn clean_project(project_dir: &Path, options: CleanOptions) -> Result<CleanResult> {
    let _ = (project_dir, options);
    todo!("port project/clean.ts:cleanProject")
}
