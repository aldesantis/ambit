//! Pruning: removes what the last install owned and this one does not.
//!
//! Without it, install would be purely additive: narrowing `requires` in `ambit.yml` would change
//! nothing on disk, and the harness would keep loading a skill the project no longer selects.
//!
//! The input is `.ambit/state.json`, never the filesystem. Ambit deletes only what it recorded
//! creating, so a hand-written skill beside ambit's is never touched, and dropping a harness from
//! `harnesses` prunes that adapter's artifacts automatically (they are simply absent from the
//! plan).
//!
//! Granularity follows the artifact kind, same as ownership: a skill directory goes whole; a
//! harness config file loses only ambit's stale keys and stays where it is, because files like
//! `.mcp.json` are co-owned. The directories that held pruned skills stay too: `.claude/skills` and
//! `.claude` belong to the harness.
//!
//! Pruning runs after materialization and before state is rewritten, so a failure here is
//! retryable rather than destructive: state still owns everything, and the next install prunes the
//! same set again. A failed `apply` leaves the previous install intact instead of half-dismantled.
//!
//! Deciding what to remove ([`plan_prune`]) is split from removing it ([`prune_artifacts`])
//! because three commands need the same answer for different reasons: `install` acts on it,
//! `--dry-run` prints it, and `ambit prune` acts on it without materializing anything first.
//! `clean` is the same decision against an empty plan, which is why nothing here has its own notion
//! of "remove everything".

use std::path::Path;

use indexmap::{IndexMap, IndexSet};

use crate::errors::{Result, config_error};
use crate::harness::adapter::PlannedArtifact;
use crate::model::documents::{DocumentFormat, DocumentShape, driver_for, read_document_text};
use crate::model::state::{ArtifactKind, OwnedArtifact, STATE_DIRNAME, STATE_FILENAME, State};
use crate::util::cmp::js_cmp;
use crate::util::fs::{self, EntryKind};
use crate::util::path::join;

/// One artifact pruning removed, mirroring the state entry that authorized the removal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrunedArtifact {
    /// Project-relative, `/`-separated.
    pub path: String,
    pub kind: ArtifactKind,
    /// For `harness-config`: the dotted keys taken out of the file, in the order they were removed.
    pub managed_keys: Option<Vec<String>>,
    /// For `harness-config`: how the file is written, carried over from the state entry.
    ///
    /// Pruning acts from state alone, so the format has to travel with the removal rather than be
    /// re-derived from a project that may no longer resolve.
    pub format: Option<DocumentFormat>,
    /// For `harness-config`: how the managed section is laid out, carried over from the state
    /// entry.
    ///
    /// Not implied by `format`: a `.claude/settings.json` and a `.mcp.json` are both JSON, but
    /// reading the first with the map driver would look for a `<Event>@<digest>` key among the
    /// event names, find none, and prune nothing at all.
    pub shape: Option<DocumentShape>,
}

/// The paths the new plan writes whole; a prior one absent from this set is stale.
///
/// Every kind owned as a path must be listed, not only the ones that were here first. A kind left
/// out is a path the plan does write that this set would claim it doesn't, so pruning would delete
/// it right after `apply` created it, and the next install would recreate it forever.
fn planned_paths(plan: &[PlannedArtifact]) -> IndexSet<&str> {
    plan.iter()
        .filter(|artifact| artifact.kind() != ArtifactKind::HarnessConfig)
        .map(PlannedArtifact::path)
        .collect()
}

/// The managed keys the new plan writes, by config file.
///
/// A file the plan does not mention at all is missing from the map, and the caller reads that as
/// the empty set: a bundle that selects no servers plans no `.mcp.json` artifact, and every key
/// prior state claims there is therefore stale.
fn planned_keys(plan: &[PlannedArtifact]) -> IndexMap<&str, IndexSet<&str>> {
    let mut by_file: IndexMap<&str, IndexSet<&str>> = IndexMap::new();

    for artifact in plan {
        if let PlannedArtifact::HarnessConfig(config) = artifact {
            by_file
                .entry(config.path.as_str())
                .or_default()
                .extend(config.managed_keys.iter().map(String::as_str));
        }
    }

    by_file
}

/// Splits a state entry's dotted key back into the section and the key within it.
///
/// Split at the first dot: a section is a name ambit chose and never contains one, whereas the key
/// is an entity name that can (`mcpServers.acme.internal` is one server called `acme.internal`,
/// not a nested object).
///
/// # Errors
///
/// Exit 2 for a key naming no section, which this build cannot have written.
fn split_managed_key<'k>(key: &'k str, file: &str) -> Result<(&'k str, &'k str)> {
    match key.find('.') {
        Some(dot) if dot > 0 && dot != key.len() - 1 => Ok((&key[..dot], &key[dot + 1..])),
        _ => Err(config_error(
            format!("cannot prune \"{key}\" from {file}"),
            [
                format!(
                    "{STATE_DIRNAME}/{STATE_FILENAME} records it as a managed key, but it names no section"
                ),
                format!(
                    "correct that entry, or delete {STATE_DIRNAME}/{STATE_FILENAME} and run `ambit install --adopt`"
                ),
            ],
        )),
    }
}

/// What pruning a plan against prior state would remove, without touching disk.
///
/// Answerable from state and the plan alone, which is what lets `--dry-run` and `ambit prune`
/// report the same set install would act on.
///
/// Managed keys are validated here, before anything is deleted, so a state entry this build could
/// not have written is exit 2 with the project untouched, rather than failing after the first skill
/// directory is already gone.
///
/// `plan` is every artifact the run writes; empty means "keep nothing", which is `clean`. `prior`
/// is the state from the last install, the only thing that authorizes a removal. Returns the
/// removals, ordered by path and then by key, so two identical runs read identically.
///
/// # Errors
///
/// Exit 2 for a managed key that names no section, before anything is deleted.
pub fn plan_prune(plan: &[PlannedArtifact], prior: &State) -> Result<Vec<PrunedArtifact>> {
    let kept_paths = planned_paths(plan);
    let kept_keys = planned_keys(plan);
    let no_keys = IndexSet::new();
    let mut stale = Vec::new();

    // Sorted by path so the order removals happen in (and are reported in) is a function of the
    // artifacts and not of however state came off disk.
    let mut artifacts: Vec<&OwnedArtifact> = prior.artifacts.iter().collect();

    artifacts.sort_by(|a, b| js_cmp(&a.path, &b.path));

    for artifact in artifacts {
        if artifact.kind == ArtifactKind::HarnessConfig {
            let kept = kept_keys.get(artifact.path.as_str()).unwrap_or(&no_keys);
            let mut keys: Vec<String> = artifact.managed_keys.clone().unwrap_or_default();

            keys.sort_by(|a, b| js_cmp(a, b));
            keys.retain(|key| !kept.contains(key.as_str()));

            if keys.is_empty() {
                continue;
            }

            for key in &keys {
                split_managed_key(key, &artifact.path)?;
            }

            stale.push(PrunedArtifact {
                path: artifact.path.clone(),
                kind: artifact.kind,
                managed_keys: Some(keys),
                format: artifact.format,
                shape: artifact.shape,
            });
            continue;
        }

        if kept_paths.contains(artifact.path.as_str()) {
            continue;
        }

        stale.push(PrunedArtifact {
            path: artifact.path.clone(),
            kind: artifact.kind,
            managed_keys: None,
            format: None,
            shape: None,
        });
    }

    Ok(stale)
}

/// What state records once `pruned` is gone: the entries that survive, in their prior order.
///
/// Install has no need for this: it writes the artifacts it just applied. A standalone `prune` has
/// to subtract instead, and it subtracts the planned removals rather than the writes that happened,
/// so a key state claimed in a file someone had already emptied by hand stops being claimed too.
pub fn remaining_artifacts(prior: &State, pruned: &[PrunedArtifact]) -> Vec<OwnedArtifact> {
    let removed: IndexMap<&str, &PrunedArtifact> = pruned
        .iter()
        .map(|artifact| (artifact.path.as_str(), artifact))
        .collect();
    let mut kept = Vec::new();

    for artifact in &prior.artifacts {
        let Some(gone) = removed.get(artifact.path.as_str()) else {
            kept.push(artifact.clone());
            continue;
        };

        if artifact.kind != ArtifactKind::HarnessConfig {
            continue;
        }

        let gone_keys = gone.managed_keys.as_deref().unwrap_or_default();
        let keys: Vec<String> = artifact
            .managed_keys
            .iter()
            .flatten()
            .filter(|key| !gone_keys.contains(key))
            .cloned()
            .collect();

        if !keys.is_empty() {
            kept.push(OwnedArtifact {
                managed_keys: Some(keys),
                ..artifact.clone()
            });
        }
    }

    kept
}

/// Takes `stale` out of one co-owned config file.
///
/// The document is re-read here rather than carried over from planning, because `apply` has
/// already merged this run's own keys into it, and writing a pre-`apply` snapshot back would undo
/// them.
///
/// A key already gone (the file deleted by hand, the server removed by hand) is not an error and
/// not a write: an install that prunes nothing must leave the file byte-identical, and recreating a
/// file someone deleted would be worse than leaving it absent.
///
/// `format` is how the file is written, as prior state recorded it. Pruning runs from state alone,
/// so the format must come from there rather than from re-resolving the project. `shape` is how
/// the managed section is laid out, from state for the same reason; absent reads as `Map`, which is
/// what every artifact written before the field existed was.
///
/// # Errors
///
/// Exit 2 if the file exists but cannot be parsed, or for a managed key that names no section.
fn prune_config_keys(
    project_dir: &Path,
    file: &str,
    stale: &[String],
    format: DocumentFormat,
    shape: Option<DocumentShape>,
) -> Result<Option<PrunedArtifact>> {
    let target = join(project_dir, file);
    let driver = driver_for(format, shape.unwrap_or(DocumentShape::Map), None)?;
    let mut text = read_document_text(&target, file)?;
    let mut removed = Vec::new();

    for key in stale {
        let (section, name) = split_managed_key(key, file)?;
        let Some(next) = driver.remove_keys(text.as_deref(), section, &[name.to_owned()], file)?
        else {
            continue;
        };

        text = Some(next);
        removed.push(key.clone());
    }

    let Some(text) = text.filter(|_| !removed.is_empty()) else {
        return Ok(None);
    };

    fs::write_text(&target, &text)?;

    Ok(Some(PrunedArtifact {
        path: file.to_owned(),
        kind: ArtifactKind::HarnessConfig,
        managed_keys: Some(removed),
        format: None,
        shape: None,
    }))
}

/// Whether a stale path still names the thing ambit recorded creating.
///
/// Exists for the migration to the shared skills directory. An install from before it owned
/// `.claude/skills/<name>` directories; afterwards `.claude/skills` is a symlink to
/// `.agents/skills`, so those old paths resolve straight through it into the shared directory,
/// where the skills this run just installed now live. Deleting them would delete the new install.
///
/// A symlinked ancestor means the directory ambit owned no longer exists as a directory, so there
/// is nothing at that path left to remove: whatever is reachable through it now belongs to whatever
/// the link points at. It is still reported as pruned, because state should stop claiming it either
/// way.
///
/// Only ancestors are inspected. The artifact itself is often a symlink (one of the two
/// materialization modes) and removal unlinks it without following it.
fn owned_path_intact(project_dir: &Path, relative: &str) -> bool {
    let segments: Vec<&str> = relative.split('/').collect();
    let mut current = project_dir.to_path_buf();

    for segment in &segments[..segments.len() - 1] {
        current = join(&current, segment);

        match fs::lstat_kind(&current) {
            Ok(EntryKind::Symlink) => return false,
            // An ancestor that is not there at all means the artifact is not either, and a forced
            // removal of it would be a no-op, so there is no reason to treat it as the link case.
            Ok(EntryKind::Missing) | Err(_) => return true,
            Ok(_) => {}
        }
    }

    true
}

/// Removes every owned artifact the new plan no longer writes.
///
/// Call it once with every adapter's plan flattened together: an artifact one adapter now writes
/// may be one prior state recorded under another, and pruning per adapter would delete it and then
/// rewrite it. `prior` is the state from the last install, the only thing that authorizes a
/// deletion.
///
/// Returns what was actually removed, ordered by path and then by key. It is a subset of
/// [`plan_prune`]'s answer: a key already absent from its file is a removal with nothing left to
/// do.
///
/// # Errors
///
/// Exit 2 for a co-owned config file that cannot be parsed, or a managed key state records in a
/// form this build cannot act on. Nothing has been deleted in either case; state is still intact,
/// so the next install prunes the same set again.
pub fn prune_artifacts(
    project_dir: &Path,
    plan: &[PlannedArtifact],
    prior: &State,
) -> Result<Vec<PrunedArtifact>> {
    let mut pruned = Vec::new();

    for artifact in plan_prune(plan, prior)? {
        if artifact.kind == ArtifactKind::HarnessConfig {
            let removed = prune_config_keys(
                project_dir,
                &artifact.path,
                artifact.managed_keys.as_deref().unwrap_or_default(),
                artifact.format.unwrap_or(DocumentFormat::Json),
                artifact.shape,
            )?;

            pruned.extend(removed);
            continue;
        }

        // Forced, because an artifact someone already deleted is a prune that has nothing left to
        // do, not a failure; a directory goes whole, and a symlink is unlinked without following
        // it.
        if owned_path_intact(project_dir, &artifact.path) {
            fs::rm_rf(&join(project_dir, &artifact.path))?;
        }

        pruned.push(artifact);
    }

    Ok(pruned)
}
