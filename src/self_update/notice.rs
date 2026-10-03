//! The one line that tells a user a newer ambit exists.
//!
//! A dependency manager is run in loops and in scripts, so this is built to be invisible until it
//! has something to say and to cost nothing when it does not:
//!
//! - **At most one request a day.** The answer is cached under the same root the git cache uses,
//!   and a fresh cache answers without touching the network at all.
//! - **Silent unless a person is watching.** No notice when stderr is not a terminal, under CI,
//!   with `--json` or `--offline`, when [`OPT_OUT_VAR`] is set, or when the command being run is
//!   the update itself.
//! - **Never a failure.** Every error is swallowed. Being unable to check for a new version is not
//!   a reason for a command that already succeeded to say anything at all.
//!
//! The timestamp is written whether or not the check succeeded, so a machine that is offline makes
//! one failed request a day rather than one per command.

use std::path::PathBuf;
use std::time::Duration;

use crate::model::git::cache_root;
use crate::self_update::release::{Http, is_newer, latest_tag};
use crate::util::env::Env;
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};
use crate::util::path::join;
use crate::util::text::js_trim;
use crate::version::VERSION;

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

/// What the cache file holds. `latest` is absent when the last check could not reach GitHub.
struct NoticeCache {
    /// Milliseconds since the Unix epoch, stored as a JSON number.
    checked_at: u64,
    latest: Option<String>,
}

fn is_set(env: &Env, name: &str) -> bool {
    env.get(name)
        .is_some_and(|value| !js_trim(value).is_empty())
}

/// Whether this run may check at all, from the invocation alone.
///
/// Separate from the check so the guards can be read as a list and tested as one. `--json` is here
/// because a machine reading ambit's output did not ask for advice, and `--offline` because the
/// flag is a statement that this run must not reach the network.
pub fn should_check(context: &NoticeContext<'_>) -> bool {
    if is_set(context.env, OPT_OUT_VAR) || is_set(context.env, "CI") || !context.is_tty {
        return false;
    }

    !context
        .argv
        .iter()
        .any(|arg| arg == "--json" || arg == "--offline" || arg == "self-update")
}

fn cache_file(env: &Env) -> PathBuf {
    join(&cache_root(env), NOTICE_CACHE_FILE)
}

fn read_cache(env: &Env) -> Option<NoticeCache> {
    let text = crate::util::fs::read_text(&cache_file(env)).ok()?;
    let parsed = crate::util::json::parse(&text).ok()?;
    let record = parsed.as_object()?;
    let checked_at = record.get("checkedAt")?.as_f64()?;

    Some(NoticeCache {
        // A JSON number from any writer; a negative or fractional one still orders correctly once
        // truncated, and one from the future just means "fresh".
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        checked_at: checked_at.max(0.0) as u64,
        latest: record
            .get("latest")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
    })
}

fn write_cache(env: &Env, cache: &NoticeCache) {
    let mut record = JsonObject::new();
    record.insert("checkedAt".to_owned(), cache.checked_at.into());

    if let Some(latest) = &cache.latest {
        record.insert("latest".to_owned(), latest.clone().into());
    }

    let file = cache_file(env);
    let text = format!("{}\n", stringify_pretty(&JsonValue::Object(record)));

    // An unwritable cache costs one request per run, which is not worth failing a command over.
    let _ = file
        .parent()
        .map_or(Ok(()), crate::util::fs::mkdir_p)
        .and_then(|()| crate::util::fs::write_text(&file, &text));
}

/// The tag of the newest release, from the cache when it is fresh and from GitHub when it is not.
fn newest_release(context: &NoticeContext<'_>) -> Option<String> {
    if let Some(cached) = read_cache(context.env)
        && context.now.saturating_sub(cached.checked_at) < CHECK_INTERVAL_MS
    {
        return cached.latest;
    }

    let latest = latest_tag(context.http, CHECK_TIMEOUT).ok();

    write_cache(
        context.env,
        &NoticeCache {
            checked_at: context.now,
            latest: latest.clone(),
        },
    );

    latest
}

/// The line to print, or `None` when there is nothing to say. Never fails: every problem means
/// there is nothing to say.
pub fn update_notice(context: &NoticeContext<'_>) -> Option<String> {
    if !should_check(context) {
        return None;
    }

    let latest = newest_release(context)?;

    if !is_newer(VERSION, &latest) {
        return None;
    }

    Some(format!(
        "ambit {latest} is available; you are on {VERSION}. To upgrade, run `ambit self-update`."
    ))
}

#[cfg(test)]
mod tests;
