//! What ambit may overwrite: checking a plan against prior ownership before anything is written.

use indexmap::IndexSet;

use crate::errors::Result;
use crate::harness::adapter::PlannedArtifact;
use crate::model::state::State;

/// How an install was told to treat a target ambit does not own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnershipOptions {
    /// `--adopt`: take ownership of what is already there instead of refusing it.
    pub adopt: bool,
}

/// The dotted keys prior state records as ambit's within one config file.
///
/// Unioned across every artifact naming that path, so ownership survives two adapters writing into
/// one file: the path alone never grants it, because the file is co-owned.
pub fn owned_keys(prior: &State, file: &str) -> IndexSet<String> {
    let _ = (prior, file);
    todo!("port project/ownership.ts:ownedKeys")
}

/// Checks a whole plan against prior ownership, and returns the ownership `apply` may act with.
///
/// Call this once, with every adapter's plan, before any adapter runs, so a project with one
/// conflict is left untouched rather than partly written. Returns `prior`, plus an owned entry for
/// every target `--adopt` just took over.
///
/// # Errors
///
/// Exit 2 naming the path or key it will not overwrite, and `--adopt` as the way to say otherwise.
pub fn authorize_plan(
    plan: &[PlannedArtifact],
    prior: &State,
    options: OwnershipOptions,
) -> Result<State> {
    let _ = (plan, prior, options);
    todo!("port project/ownership.ts:authorizePlan")
}
