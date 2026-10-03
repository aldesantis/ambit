//! `ambit status`: comparing what is installed against what install would write.

use std::path::Path;

use crate::errors::Result;
use crate::harness::adapter::PlannedArtifact;
use crate::model::state::{ArtifactKind, State};
use crate::util::env::Env;
use crate::util::string_enum;

string_enum! {
    /// What comparing one artifact against the project concluded.
    ///
    /// - `Missing`: resolution wants it and nothing is installed.
    /// - `Modified`: it is installed and owned, but its contents are not what install would write.
    /// - `Ok`: install would write exactly what is already there.
    /// - `Stale`: ambit owns it and resolution no longer selects it, so install would prune it.
    /// - `Unowned`: something is there that ambit did not create, which install refuses to
    ///   overwrite.
    pub enum ArtifactState {
        Missing => "missing",
        Modified => "modified",
        Ok => "ok",
        Stale => "stale",
        Unowned => "unowned",
    }
}

/// Every artifact state, in declaration order.
pub const ARTIFACT_STATES: &[ArtifactState] = ArtifactState::ALL;

/// One artifact's verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusArtifact {
    /// Project-relative, `/`-separated.
    pub path: String,
    pub kind: ArtifactKind,
    pub state: ArtifactState,
    /// One line naming what differs, empty when `ok`.
    pub detail: String,
}

/// What `status` found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectStatus {
    /// Every artifact resolution wants plus every one state still owns, sorted by path.
    pub artifacts: Vec<StatusArtifact>,
}

/// How a status comparison was asked to behave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusOptions {
    /// Resolve from the catalog cache alone, failing rather than fetching.
    pub offline: bool,
}

/// Everything `status` would report, which is everything install would change.
pub fn status_drift(status: &ProjectStatus) -> Vec<StatusArtifact> {
    let _ = status;
    todo!("port project/status.ts:statusDrift")
}

/// Whether install would leave the project exactly as it is: the answer `--check` reports.
pub fn is_clean(status: &ProjectStatus) -> bool {
    let _ = status;
    todo!("port project/status.ts:isClean")
}

/// Compares an already-planned install against the project: the comparison without the
/// resolution.
///
/// Used by `doctor`, which needs both this verdict and the rest of `plan_install`'s output and must
/// not resolve the project twice to get them.
///
/// # Errors
///
/// Exit 2 for a target that cannot be inspected or a config file that cannot be parsed.
pub fn status_of_plan(plan: &[PlannedArtifact], prior: &State) -> Result<ProjectStatus> {
    let _ = (plan, prior);
    todo!("port project/status.ts:statusOfPlan")
}

/// Compares a project against what resolution now produces.
///
/// # Errors
///
/// Exit 2 for a malformed config or catalog, an unknown harness, an unreadable state file, or a
/// target that cannot be inspected; exit 3 for a resolution error; exit 4 if a fetch fails, or under
/// `--offline` when the cache cannot answer. Drift itself is never an error.
pub fn project_status(
    project_dir: &Path,
    env: &Env,
    options: StatusOptions,
) -> Result<ProjectStatus> {
    let _ = (project_dir, env, options);
    todo!("port project/status.ts:projectStatus")
}
