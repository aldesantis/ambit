//! `ambit prune` and `ambit clean`: the two commands that only remove.
//!
//! Both use `prune.rs`'s machinery without materializing anything; what differs is what they keep.
//! `prune` keeps whatever the current bundle selects: it is the install-time prune without the
//! install, so narrowing `ambit.yml` doesn't require reinstalling to drop the withdrawn skills.
//! `clean` keeps nothing, so it is the same call against an empty plan rather than a second
//! traversal of state.
//!
//! `clean` resolves nothing: it answers from `.ambit/state.json` alone, so it still works on a
//! project whose catalog is unreachable, whose config no longer parses, or whose `requires` entries
//! were deleted (the usual state a project is in when someone reaches for it). `prune` cannot skip
//! resolution, because "not in the current bundle" is a question only resolution can answer.
//!
//! Only `prune` rewrites `ambit.lock`, for the same reason: having resolved, it knows what install
//! would record (the same `lock_text` install writes), and leaving the lock describing the wider
//! bundle would fail `doctor`'s lock check and `install --frozen` right after the prune that caused
//! it. `clean` has no lock to write and leaves the file alone.
//!
//! `clean` deliberately does not remove `ambit.lock` or a `.mcp.json` left holding an empty
//! `mcpServers`. Neither is ambit's (teams commit the lock; the config file is co-owned, with only
//! its keys ambit's), and ambit deletes only what it owns. The empty `.claude/skills` directory a
//! pruned skill leaves is the same: it belongs to the harness, and git does not track an empty
//! directory anyway. What `clean` does remove beyond the owned artifacts is ambit's state file, the
//! `.ambit/` directory when that leaves it empty, and its `.gitignore` blocks.

use std::path::Path;

use crate::errors::Result;
use crate::model::state::{
    OwnedArtifact, STATE_DIRNAME, STATE_VERSION, State, read_state, state_file_path, write_state,
};
use crate::project::gitignore::{
    GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE, read_gitignore_text, remove_gitignore_blocks,
    remove_gitignore_text, write_gitignore_blocks,
};
use crate::project::install::{
    InstallOptions, PlanContext, ignored_artifacts, plan_install, project_paths,
};
use crate::project::lock::write_lock_text;
use crate::project::prune::{PrunedArtifact, plan_prune, prune_artifacts, remaining_artifacts};
use crate::util::env::Env;
use crate::util::fs::{EntryKind, lstat_kind, rm_rf, rmdir_if_empty};
use crate::util::path::join;

#[cfg(test)]
mod tests;

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

/// Which files a `clean` would take a block out of, for `--dry-run`.
///
/// Uses the same reader the real removal uses, so a preview cannot disagree with what follows it,
/// including by refusing: an ambiguous block is refused here exactly as it would be there.
fn planned_gitignore_removals(project_dir: &Path) -> Result<Vec<String>> {
    let mut files = Vec::new();

    for file in [GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE] {
        let existing = read_gitignore_text(project_dir, file)?;

        if remove_gitignore_text(existing.as_deref(), file)?.is_some() {
            files.push(file.to_owned());
        }
    }

    Ok(files)
}

/// Whether anything at all sits at a path. A path that cannot be inspected counts as absent.
fn exists(target: &Path) -> bool {
    !matches!(lstat_kind(target), Ok(EntryKind::Missing) | Err(_))
}

/// Removes the owned artifacts the current bundle no longer selects.
///
/// Ownership is not authorized first, unlike an install: nothing here writes an artifact, and
/// pruning removes only what state already claims, so a project whose next `install` would refuse
/// an unowned target can still be pruned.
///
/// Writes are ordered as install orders them: filesystem, then lock, then state, then the
/// `.gitignore` block. A failure part way through leaves state still claiming what it was about to
/// give up, so the next run prunes the same set again. A run with nothing stale writes nothing, so
/// `prune` on an untouched project creates no state file, lock, or `.gitignore`.
///
/// # Errors
///
/// Exit 2 for a malformed config or catalog, an unknown harness, an unreadable state file, a
/// co-owned config file that cannot be parsed, or an ambiguous `.gitignore` block; exit 3 for a
/// resolution error; exit 4 if a fetch fails, or under `--offline` when the cache cannot answer.
pub fn prune_project(project_dir: &Path, env: &Env, options: PruneOptions) -> Result<PruneResult> {
    let planned = plan_install(
        project_dir,
        env,
        InstallOptions {
            offline: options.offline,
            ..InstallOptions::default()
        },
        &PlanContext::default(),
    )?;

    // The planned removals, not the writes that happened: what state must stop claiming, even
    // where the file already lost the key by hand.
    let stale = plan_prune(&planned.artifacts, &planned.prior)?;
    let remaining = remaining_artifacts(&planned.prior, &stale);

    if options.dry_run {
        return Ok(PruneResult {
            pruned: stale,
            remaining,
        });
    }

    if stale.is_empty() {
        return Ok(PruneResult {
            pruned: Vec::new(),
            remaining: planned.prior.artifacts,
        });
    }

    let pruned = prune_artifacts(&planned.project.root, &planned.artifacts, &planned.prior)?;

    // The bundle install would resolve is, after this prune, also the bundle on disk, so the lock
    // is `planned.lock_text` verbatim, written the same way install writes it. This keeps
    // `doctor`'s lock check clean afterwards: without it, the lock would still describe the wider
    // bundle and the project would report drift it had just finished resolving.
    write_lock_text(project_dir, &planned.lock_text)?;

    write_state(
        project_dir,
        &State {
            version: STATE_VERSION,
            harnesses: planned.harnesses,
            artifacts: remaining.clone(),
        },
    )?;
    write_gitignore_blocks(project_dir, ignored_artifacts(&planned.project, &remaining))?;

    Ok(PruneResult { pruned, remaining })
}

/// Removes everything ambit owns in a project.
///
/// `env` is read only for `HOME`, which says where a user-level install's artifacts are (see
/// [`project_paths`]).
///
/// Order: artifacts, then the `.gitignore` block, then `.ambit/`. State is removed last, one step
/// later than install puts it, because the `.gitignore` block is not free to rewrite here: an
/// ambiguous one is exit 2, and leaving state in place until after that keeps the whole command
/// retryable once the file is fixed. Nothing else reads state afterwards, so removing it last costs
/// nothing.
///
/// # Errors
///
/// Exit 2 for an unreadable state file, a co-owned config file that cannot be parsed, a managed key
/// state records in a form this build cannot act on, or a `.gitignore` whose markers are ambiguous.
/// No catalog is read and nothing is resolved, so there is no exit 3 or 4.
pub fn clean_project(project_dir: &Path, env: &Env, options: CleanOptions) -> Result<CleanResult> {
    let prior = read_state(project_dir)?;
    let state_dir = join(project_dir, STATE_DIRNAME);
    let root = project_paths(project_dir, env, None).root;

    if options.dry_run {
        return Ok(CleanResult {
            removed: plan_prune(&[], &prior)?,
            state_removed: exists(&state_file_path(project_dir)),
            gitignore_removed: planned_gitignore_removals(project_dir)?,
        });
    }

    let removed = prune_artifacts(&root, &[], &prior)?;
    let gitignore_removed = remove_gitignore_blocks(project_dir)?;

    let state_removed = exists(&state_file_path(project_dir));

    // The state file, then the directory only if that emptied it: at the home directory, `.ambit/`
    // is also the user-level project, which a clean of the home directory must not take with it.
    rm_rf(&state_file_path(project_dir))?;
    rmdir_if_empty(&state_dir)?;

    Ok(CleanResult {
        removed,
        state_removed,
        gitignore_removed,
    })
}
