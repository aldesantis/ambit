//! `ambit self-update [version]`: replace this binary with a released one.
//!
//! The one command whose subject is ambit rather than a project, which is why it takes no
//! `--project` and why the version is a positional rather than a `--version` flag.
//!
//! A named version is installed whether it is newer or older, so a bad release can be backed out
//! without hunting down the install script. Only the report says which direction it went.
//!
//! `--dry-run` prints the plan, which is the whole decision: the plan is made before anything is
//! downloaded, so it is also what every refusal comes out of.

use crate::cli::commands::{CommandContext, offline_requested};
use crate::errors::{ExitCode, Result, network_error};
use crate::self_update::release::Http;
use crate::self_update::update::SelfContext;

/// The refusal of `--offline`.
///
/// Worded for this command rather than shared with `outdated` and `update`: those refuse because
/// only a remote knows where a ref points now, and this one refuses because the bytes it installs
/// do not exist locally.
///
/// # Errors
///
/// Exit 4 when `--offline` was given.
pub fn refuses_offline_self_update_rule(ctx: &CommandContext<'_>) -> Result<()> {
    if !offline_requested(ctx) {
        return Ok(());
    }

    Err(network_error(
        "`--offline` cannot install a release",
        [
            "this command downloads a binary from GitHub, which no local cache holds",
            "run the command again without `--offline`",
        ],
    ))
}

/// What the machine looks like to self-update, built at the CLI boundary.
pub fn self_context_of(http: &dyn Http) -> SelfContext<'_> {
    let _ = http;
    todo!("port cli/handlers/self-update.ts:selfContextOf")
}

/// # Errors
///
/// Whatever planning or applying the update returns, already in the standard message shape.
pub fn self_update_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let _ = ctx;
    todo!("port cli/handlers/self-update.ts:selfUpdateHandler")
}
