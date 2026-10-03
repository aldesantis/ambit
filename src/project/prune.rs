//! Removing owned artifacts a plan no longer writes.

use std::path::Path;

use crate::errors::Result;
use crate::harness::adapter::PlannedArtifact;
use crate::model::documents::{DocumentFormat, DocumentShape};
use crate::model::state::{ArtifactKind, OwnedArtifact, State};

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
    /// entry. Not implied by `format`.
    pub shape: Option<DocumentShape>,
}

/// What pruning a plan against prior state would remove, without touching disk.
///
/// `plan` is every artifact the run writes; empty means "keep nothing", which is `clean`. Returns
/// the removals, ordered by path and then by key.
///
/// # Errors
///
/// Exit 2 for a managed key that names no section, before anything is deleted.
pub fn plan_prune(plan: &[PlannedArtifact], prior: &State) -> Result<Vec<PrunedArtifact>> {
    let _ = (plan, prior);
    todo!("port project/prune.ts:planPrune")
}

/// What state records once `pruned` is gone: the entries that survive, in their prior order.
pub fn remaining_artifacts(prior: &State, pruned: &[PrunedArtifact]) -> Vec<OwnedArtifact> {
    let _ = (prior, pruned);
    todo!("port project/prune.ts:remainingArtifacts")
}

/// Removes every owned artifact the new plan no longer writes.
///
/// Call it once with every adapter's plan flattened together. Returns what was actually removed,
/// ordered by path and then by key: a subset of [`plan_prune`]'s answer.
///
/// # Errors
///
/// Exit 2 for a co-owned config file that cannot be parsed, or a managed key state records in a
/// form this build cannot act on. Nothing has been deleted in either case.
pub fn prune_artifacts(
    project_dir: &Path,
    plan: &[PlannedArtifact],
    prior: &State,
) -> Result<Vec<PrunedArtifact>> {
    let _ = (project_dir, plan, prior);
    todo!("port project/prune.ts:pruneArtifacts")
}
