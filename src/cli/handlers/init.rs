//! `ambit init`: scaffold a project, which is also a catalog.

use crate::cli::commands::CommandContext;
use crate::errors::{ExitCode, Result};

/// # Errors
///
/// Whatever the command's own operation returns, already in the standard message shape.
pub fn init_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let _ = ctx;
    todo!("port cli/handlers/init.ts:initHandler")
}
