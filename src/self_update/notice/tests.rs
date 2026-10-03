//! The update notice: when it stays quiet, and how rarely it asks.
//!
//! Most of these assert a negative, because a notice is only tolerable if it is nearly always
//! absent. Every guard is checked one at a time against an otherwise valid context, so a guard that
//! stopped working could not hide behind another one still holding.
//!
//! The cache cases assert on whether the [`Http`] was called at all, not on the returned line: the
//! promise of a daily check is a promise about requests, and a version comparison that happened to
//! be right would say nothing about how it was reached.

use std::path::Path;

use super::*;
use crate::model::git::CACHE_DIRNAME;
use crate::self_update::fake_http::{Canned, FakeHttp};
use crate::test_support::tempdir;

const NEWER: &str = "v99.0.0";
const NOW: u64 = 1_760_000_000_000;

/// A GitHub that names [`NEWER`] as latest.
fn counting() -> FakeHttp {
    FakeHttp::new(|url| {
        if !url.ends_with("/releases/latest") {
            return Ok(Canned::status(404));
        }

        Ok(Canned::redirect(&format!(
            "https://github.com/aldesantis/ambit/releases/tag/{NEWER}"
        )))
    })
}

/// A GitHub that is unreachable.
fn failing() -> FakeHttp {
    FakeHttp::new(|_| Err("offline".to_owned()))
}

fn env_with(cache_home: &Path, extra: &[(&str, &str)]) -> Env {
    let mut env = Env::new();
    env.insert(
        "XDG_CACHE_HOME".to_owned(),
        cache_home.to_string_lossy().into_owned(),
    );

    for (name, value) in extra {
        env.insert((*name).to_owned(), (*value).to_owned());
    }

    env
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|&arg| arg.to_owned()).collect()
}

fn context<'a>(env: &'a Env, argv: &'a [String], http: &'a FakeHttp) -> NoticeContext<'a> {
    NoticeContext {
        env,
        argv,
        is_tty: true,
        now: NOW,
        http,
    }
}

fn cache_file(cache_home: &Path) -> PathBuf {
    cache_home.join(CACHE_DIRNAME).join(NOTICE_CACHE_FILE)
}

fn write_cache(cache_home: &Path, record: &str) {
    std::fs::create_dir_all(cache_home.join(CACHE_DIRNAME)).expect("create the cache");
    std::fs::write(cache_file(cache_home), record).expect("write the cache");
}

fn read_cache(cache_home: &Path) -> String {
    crate::util::fs::read_text(&cache_file(cache_home)).expect("read the cache")
}

#[test]
fn reports_a_newer_release_and_names_the_command_that_installs_it() {
    let home = tempdir();
    let env = env_with(home.path(), &[]);
    let args = argv(&["status"]);
    let http = counting();

    assert_eq!(
        update_notice(&context(&env, &args, &http)),
        Some(format!(
            "ambit {NEWER} is available; you are on {VERSION}. To upgrade, run `ambit self-update`."
        ))
    );
}

#[test]
fn says_nothing_when_the_latest_release_is_the_one_running() {
    let home = tempdir();
    let env = env_with(home.path(), &[]);
    let args = argv(&["status"]);
    let http = FakeHttp::new(|_| {
        Ok(Canned::redirect(&format!(
            "https://github.com/aldesantis/ambit/releases/tag/v{VERSION}"
        )))
    });

    assert_eq!(update_notice(&context(&env, &args, &http)), None);
}

/// Asserts the notice is silent for this invocation, and that it asked nothing.
fn assert_silent(extra_env: &[(&str, &str)], args: &[&str], is_tty: bool) {
    let home = tempdir();
    let env = env_with(home.path(), extra_env);
    let args = argv(args);
    let http = counting();
    let context = NoticeContext {
        is_tty,
        ..context(&env, &args, &http)
    };

    assert_eq!(update_notice(&context), None);
    assert_eq!(http.calls.get(), 0);
}

#[test]
fn says_nothing_when_ambit_no_update_check_is_set() {
    assert_silent(&[("AMBIT_NO_UPDATE_CHECK", "1")], &["status"], true);
}

#[test]
fn says_nothing_under_ci() {
    assert_silent(&[("CI", "true")], &["status"], true);
}

#[test]
fn says_nothing_when_stderr_is_not_a_terminal() {
    assert_silent(&[], &["status"], false);
}

#[test]
fn says_nothing_to_a_machine_reading_the_output() {
    assert_silent(&[], &["status", "--json"], true);
}

#[test]
fn says_nothing_when_the_run_was_told_not_to_reach_the_network() {
    assert_silent(&[], &["install", "--offline"], true);
}

#[test]
fn says_nothing_during_the_update_itself() {
    assert_silent(&[], &["self-update"], true);
}

#[test]
fn checks_when_a_guard_variable_is_set_to_nothing() {
    let home = tempdir();
    let env = env_with(home.path(), &[("AMBIT_NO_UPDATE_CHECK", " "), ("CI", "")]);
    let args = argv(&["status"]);
    let http = counting();

    assert!(update_notice(&context(&env, &args, &http)).is_some());
    assert_eq!(http.calls.get(), 1);
}

#[test]
fn answers_from_a_cache_younger_than_the_interval_without_asking() {
    let home = tempdir();
    write_cache(
        home.path(),
        &format!(
            "{{\"checkedAt\":{},\"latest\":\"{NEWER}\"}}",
            NOW - CHECK_INTERVAL_MS + 1000
        ),
    );
    let env = env_with(home.path(), &[]);
    let args = argv(&["status"]);
    let http = counting();

    assert!(
        update_notice(&context(&env, &args, &http))
            .expect("a notice")
            .contains(NEWER)
    );
    assert_eq!(http.calls.get(), 0);
}

#[test]
fn asks_again_once_the_cache_is_older_than_the_interval() {
    let home = tempdir();
    write_cache(
        home.path(),
        &format!(
            "{{\"checkedAt\":{},\"latest\":\"v0.0.1\"}}",
            NOW - CHECK_INTERVAL_MS - 1
        ),
    );
    let env = env_with(home.path(), &[]);
    let args = argv(&["status"]);
    let http = counting();

    assert!(
        update_notice(&context(&env, &args, &http))
            .expect("a notice")
            .contains(NEWER)
    );
    assert_eq!(http.calls.get(), 1);
    assert_eq!(
        read_cache(home.path()),
        format!("{{\n  \"checkedAt\": {NOW},\n  \"latest\": \"{NEWER}\"\n}}\n")
    );
}

#[test]
fn records_the_attempt_even_when_it_fails_so_an_offline_machine_asks_once_a_day() {
    let home = tempdir();
    let env = env_with(home.path(), &[]);
    let args = argv(&["status"]);
    let http = failing();

    assert_eq!(update_notice(&context(&env, &args, &http)), None);
    assert_eq!(http.calls.get(), 1);
    assert_eq!(
        read_cache(home.path()),
        format!("{{\n  \"checkedAt\": {NOW}\n}}\n")
    );

    assert_eq!(update_notice(&context(&env, &args, &http)), None);
    assert_eq!(http.calls.get(), 1);
}

#[test]
fn ignores_a_cache_file_that_is_not_the_shape_it_wrote() {
    let home = tempdir();
    write_cache(home.path(), "{\"checkedAt\":\"yesterday\"}");
    let env = env_with(home.path(), &[]);
    let args = argv(&["status"]);
    let http = counting();

    assert!(
        update_notice(&context(&env, &args, &http))
            .expect("a notice")
            .contains(NEWER)
    );
    assert_eq!(http.calls.get(), 1);
}

#[test]
fn says_nothing_and_fails_nothing_when_the_cache_cannot_be_written() {
    let home = tempdir();
    // A file where the cache directory should be.
    std::fs::write(home.path().join(CACHE_DIRNAME), "").expect("block the cache");
    let env = env_with(home.path(), &[]);
    let args = argv(&["status"]);
    let http = failing();

    assert_eq!(update_notice(&context(&env, &args, &http)), None);
    assert_eq!(http.calls.get(), 1);
}
