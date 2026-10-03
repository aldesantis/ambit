//! The once-a-day "a newer ambit is available" line after a successful command.

use std::time::Duration;

use crate::self_update::release::Http;
use crate::util::env::Env;

/// Where the last check is remembered, beside the git cache's `repos/` and `sources/`.
pub const NOTICE_CACHE_FILE: &str = "self-update.json";

/// How long an answer is reused before asking again, in milliseconds.
pub const CHECK_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;

/// How long the check may take. Short: it runs after a command the user is waiting to end.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// Set to anything non-empty to never check.
pub const OPT_OUT_VAR: &str = "AMBIT_NO_UPDATE_CHECK";

/// What one command run needs to know to decide whether, and what, to report.
#[derive(Clone, Copy)]
pub struct NoticeContext<'a> {
    pub env: &'a Env,
    /// The arguments the user typed, without the program name.
    pub argv: &'a [String],
    /// Whether stderr is a terminal.
    pub is_tty: bool,
    /// Milliseconds since the Unix epoch.
    pub now: u64,
    pub http: &'a dyn Http,
}

/// Whether this run may check at all, from the invocation alone.
///
/// Separate from the check so the guards can be read as a list and tested as one. `--json` is here
/// because a machine reading ambit's output did not ask for advice, and `--offline` because the
/// flag is a statement that this run must not reach the network.
pub fn should_check(context: &NoticeContext<'_>) -> bool {
    let _ = context;
    todo!("port self/notice.ts:shouldCheck")
}

/// The line to print, or `None` when there is nothing to say. Never fails: every problem means
/// there is nothing to say.
pub fn update_notice(context: &NoticeContext<'_>) -> Option<String> {
    let _ = context;
    todo!("port self/notice.ts:updateNotice")
}
