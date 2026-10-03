//! `ambit update [<catalog>...]`: move the pins, write the lock, materialize, prune.

use crate::cli::commands::CommandContext;
use crate::errors::{ExitCode, Result};

/// # Errors
///
/// Whatever the command's own operation returns, already in the standard message shape.
pub fn update_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let _ = ctx;
    todo!("port cli/handlers/update.ts:updateHandler")
}
