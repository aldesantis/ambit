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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneOptions {
    pub offline: bool,
    pub dry_run: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneResult {
    pub pruned: Vec<PrunedArtifact>,
    pub remaining: Vec<OwnedArtifact>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanOptions {
    pub dry_run: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanResult {
    pub removed: Vec<PrunedArtifact>,
    pub state_removed: bool,
    pub gitignore_removed: Vec<String>,
}

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

fn exists(target: &Path) -> bool {
    !matches!(lstat_kind(target), Ok(EntryKind::Missing) | Err(_))
}

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

    // State is written after the removals so a failure leaves it claiming them for the next run.
    let pruned = prune_artifacts(&planned.project.root, &planned.artifacts, &planned.prior)?;

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

    // State goes last so an ambiguous `.gitignore` leaves the command retryable. At the home
    // directory `.ambit/` is also the user-level project, so it is removed only once empty.
    rm_rf(&state_file_path(project_dir))?;
    rmdir_if_empty(&state_dir)?;

    Ok(CleanResult {
        removed,
        state_removed,
        gitignore_removed,
    })
}
