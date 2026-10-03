//! `ambit clean`: remove everything ambit owns.

use crate::cli::commands::CommandContext;
use crate::errors::{ExitCode, Result};

/// # Errors
///
/// Whatever the command's own operation returns, already in the standard message shape.
pub fn clean_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let _ = ctx;
    todo!("port cli/handlers/clean.ts:cleanHandler")
}
