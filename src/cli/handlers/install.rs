//! `ambit install`: resolve, write the lock, materialize, record ownership.
//!
//! Output names artifacts by their project-relative path, so it is comparable between machines.
//! The lock is not among them: it's a record of the resolution, not an owned artifact.
//!
//! `--dry-run` prints the same two sections the install would print, plus what only a preview can
//! usefully say: what install would remove, and whether `ambit.lock` and each managed `.gitignore`
//! block would change.
//!
//! A hook a configured harness cannot express is a warning on stderr, and exit stays 0. Stderr
//! because stdout is the report a script parses; a warning, not an error, because the hook did
//! install everywhere else.

use crate::cli::commands::CommandContext;
use crate::errors::{ExitCode, Result};
use crate::harness::adapter::SkippedHook;
use crate::util::json::JsonObject;

/// One line per skipped hook, named the way its declaration names it.
///
/// Shared with `ambit update`, which ends in an install and owes the same warning.
pub fn skip_warnings(skipped: &[SkippedHook]) -> Vec<String> {
    let _ = skipped;
    todo!("port cli/handlers/install.ts:skipWarnings")
}

/// One skipped hook as a JSON record. Carries the reason kind, not the sentence.
pub fn skip_json(skipped: &SkippedHook) -> JsonObject {
    let _ = skipped;
    todo!("port cli/handlers/install.ts:skipJson")
}

/// # Errors
///
/// Whatever the install returns, already in the standard message shape.
pub fn install_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let _ = ctx;
    todo!("port cli/handlers/install.ts:installHandler")
}
