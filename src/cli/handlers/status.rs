//! `ambit status`: compare what is installed against what resolve produces.

use crate::cli::commands::CommandContext;
use crate::errors::{ExitCode, Result};

/// # Errors
///
/// Whatever the command's own operation returns, already in the standard message shape.
pub fn status_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let _ = ctx;
    todo!("port cli/handlers/status.ts:statusHandler")
}
