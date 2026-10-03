//! `ambit outdated`: where every catalog's pin stands, and what moving it would bring.
//!
//! Four sections rather than a list of stale catalogs: the first says which pins have somewhere to
//! go, and the three after it say what going there would actually change.
//!
//! Exit 0 whatever it finds: being behind is a fact, not a failure.

use crate::cli::commands::{CommandContext, offline_requested};
use crate::errors::{ExitCode, Result, network_error};
use crate::project::update::UpdatePlan;

/// Both `outdated`'s and `update`'s refusal of `--offline`.
///
/// A rule, not a check inside the handler, so it is enforced before dispatch and a run that cannot
/// mean anything never starts. Only the remote knows where a branch points now, and a cached commit
/// reported as current is worse than no report at all.
///
/// # Errors
///
/// Exit 4 when `--offline` was given.
pub fn refuses_offline_rule(ctx: &CommandContext<'_>) -> Result<()> {
    if !offline_requested(ctx) {
        return Ok(());
    }

    Err(network_error(
        "`--offline` cannot answer where a ref points now",
        [
            "this command asks each catalog's remote for its current commit, which the cache cannot know",
            "run the command again without `--offline`",
        ],
    ))
}

/// The four sections, catalogs first.
///
/// Every configured catalog is listed, not only the moved ones: "your other three are current" is
/// part of the answer.
pub fn plan_text(plan: &UpdatePlan) -> Vec<String> {
    let _ = plan;
    todo!("port cli/handlers/outdated.ts:planText")
}

/// # Errors
///
/// Whatever checking the pins returns, already in the standard message shape.
pub fn outdated_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let _ = ctx;
    todo!("port cli/handlers/outdated.ts:outdatedHandler")
}
