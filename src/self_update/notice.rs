use std::path::PathBuf;
use std::time::Duration;

use crate::model::git::cache_root;
use crate::self_update::release::{Http, is_newer, latest_tag};
use crate::util::env::Env;
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};
use crate::util::path::join;
use crate::util::text::js_trim;
use crate::version::VERSION;

pub const NOTICE_CACHE_FILE: &str = "self-update.json";

pub const CHECK_INTERVAL_MS: u64 = 24 * 60 * 60 * 1000;

pub const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

pub const OPT_OUT_VAR: &str = "AMBIT_NO_UPDATE_CHECK";

#[derive(Clone, Copy)]
pub struct NoticeContext<'a> {
    pub env: &'a Env,
    pub argv: &'a [String],
    pub is_tty: bool,
    pub now: u64,
    pub http: &'a dyn Http,
}

struct NoticeCache {
    checked_at: u64,
    latest: Option<String>,
}

fn is_set(env: &Env, name: &str) -> bool {
    env.get(name)
        .is_some_and(|value| !js_trim(value).is_empty())
}

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

    let _ = file
        .parent()
        .map_or(Ok(()), crate::util::fs::mkdir_p)
        .and_then(|()| crate::util::fs::write_text(&file, &text));
}

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
