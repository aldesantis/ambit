//! `ambit doctor`: env vars, drift, ownership.

use crate::cli::commands::CommandContext;
use crate::errors::{ExitCode, Result};

/// # Errors
///
/// Whatever the command's own operation returns, already in the standard message shape.
pub fn doctor_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let _ = ctx;
    todo!("port cli/handlers/doctor.ts:doctorHandler")
}
