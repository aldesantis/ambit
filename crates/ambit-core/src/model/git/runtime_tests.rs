//! The git runtime a library caller configures: which executable runs, how credentials reach it,
//! what is redacted, and how a run is canceled or reported on. Fake git executables are shell
//! scripts, so those cases are Unix only; the rest use the real git against `file://` remotes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::credentials::{GITHUB_TOKEN_VAR, GitFailure, classify_git_failure, with_github_token};
use super::*;
use crate::errors::ExitCode;
use crate::test_support::fixture_catalog::build_fixture_git_catalog;
use crate::test_support::{tempdir, test_env};
use crate::util::control::ProgressSink;

const TOKEN: &str = "gho_n3verPr1nted";

fn request(root: &Path, url: &str, env: Env, control: Control) -> GitFetchRequest {
    GitFetchRequest {
        url: url.to_owned(),
        subject: "catalog \"company\"".to_owned(),
        r#where: "(ambit.yml line 3)".to_owned(),
        env,
        cwd: root.to_path_buf(),
        control,
        ..GitFetchRequest::default()
    }
}

fn cancelable() -> (Arc<AtomicBool>, Control) {
    let flag = Arc::new(AtomicBool::new(false));

    (Arc::clone(&flag), Control::new(Some(flag), None))
}

/// Writes an executable shell script standing in for git.
#[cfg(unix)]
fn fake_git(dir: &Path, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = dir.join("fake-git");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write the fake git");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[cfg(unix)]
#[test]
fn runs_the_configured_git_with_the_token_only_in_its_environment() {
    let dir = tempdir();
    let root = dir.path();
    let dump = root.join("dump");
    std::fs::create_dir(&dump).expect("mkdir");
    let program = fake_git(
        root,
        r#"printf '%s\n' "$@" > "$DUMP_DIR/args"
env > "$DUMP_DIR/env"
echo "fatal: could not read Username for 'https://x-access-token:$AMBIT_GITHUB_TOKEN@github.com': terminal prompts disabled" >&2
exit 128"#,
    );

    let mut env = test_env(root);
    env.insert(GIT_PROGRAM_VAR.to_owned(), path_arg(&program));
    env.insert("DUMP_DIR".to_owned(), path_arg(&dump));
    let env = with_github_token(&env, TOKEN);

    let error = fetch_git_source(&request(
        root,
        "git@github.com:acme/private.git",
        env,
        Control::default(),
    ))
    .expect_err("the fake git fails");

    let args = std::fs::read(dump.join("args")).expect("args were dumped");
    let child_env =
        String::from_utf8(std::fs::read(dump.join("env")).expect("env was dumped")).expect("utf-8");

    assert!(!String::from_utf8_lossy(&args).contains(TOKEN));
    assert!(String::from_utf8_lossy(&args).contains("git@github.com:acme/private.git"));
    assert!(child_env.contains(&format!("{GITHUB_TOKEN_VAR}={TOKEN}")));
    assert!(child_env.contains("GIT_TERMINAL_PROMPT=0"));
    assert!(!child_env.contains(GIT_PROGRAM_VAR));

    assert_eq!(error.code, ExitCode::Network);
    assert!(!error.format().contains(TOKEN), "{}", error.format());
    assert!(
        error.detail[0].contains("https://***@github.com"),
        "{error:?}"
    );
    assert_eq!(
        classify_git_failure(&error.detail[0]),
        GitFailure::AuthRequired
    );
}

#[cfg(unix)]
#[test]
fn redacts_the_token_in_what_git_prints() {
    let dir = tempdir();
    let program = fake_git(
        dir.path(),
        r#"echo "token $AMBIT_GITHUB_TOKEN" >&2; echo "$AMBIT_GITHUB_TOKEN""#,
    );
    let mut env = with_github_token(&test_env(dir.path()), TOKEN);
    env.insert(GIT_PROGRAM_VAR.to_owned(), path_arg(&program));

    let outcome = run_git(&["status"], dir.path(), &env).expect("the fake git runs");

    assert_eq!(outcome.stderr, "token ***\n");
    assert_eq!(outcome.stdout, "***\n");
}

#[test]
fn names_the_configured_git_when_it_is_not_there() {
    let dir = tempdir();
    let missing = dir.path().join("no-such-git");
    let mut env = test_env(dir.path());
    env.insert(GIT_PROGRAM_VAR.to_owned(), path_arg(&missing));

    let error = run_git(&["--version"], dir.path(), &env).expect_err("no git there");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(
        error.message,
        format!("git is not at {}", missing.display())
    );
    assert!(error.detail[1].contains(GIT_PROGRAM_VAR));
}

#[cfg(unix)]
#[test]
fn a_cancel_kills_a_git_that_hangs() {
    let dir = tempdir();
    let program = fake_git(dir.path(), "exec sleep 30");
    let mut env = test_env(dir.path());
    env.insert(GIT_PROGRAM_VAR.to_owned(), path_arg(&program));
    let (flag, control) = cancelable();

    let canceler = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        flag.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let error = run_git_controlled(&["fetch"], dir.path(), &env, &control).expect_err("canceled");

    canceler.join().expect("the canceler finishes");

    assert_eq!(error.code, ExitCode::Canceled);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[cfg(unix)]
#[test]
fn a_canceled_clone_leaves_nothing_in_the_cache() {
    let dir = tempdir();
    let root = dir.path();
    let program = fake_git(root, "exec sleep 30");
    let mut env = test_env(root);
    env.insert(GIT_PROGRAM_VAR.to_owned(), path_arg(&program));
    let (flag, control) = cancelable();
    let url = "https://github.com/acme/skills.git";

    let canceler = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        flag.store(true, Ordering::Relaxed);
    });
    let error = fetch_git_source(&request(root, url, env.clone(), control)).expect_err("canceled");

    canceler.join().expect("the canceler finishes");

    let repos = cache_root(&env).join(REPOS_DIRNAME).join("github.com/acme");
    let leftovers = crate::util::fs::read_dir_names(&repos).unwrap_or_default();

    assert_eq!(error.code, ExitCode::Canceled);
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_fetch_canceled_up_front_runs_nothing() {
    let dir = tempdir();
    let root = dir.path();
    let fixture = build_fixture_git_catalog(&root.join("remote")).expect("build the repository");
    let env = test_env(root);
    let (flag, control) = cancelable();
    flag.store(true, Ordering::Relaxed);

    let error =
        fetch_git_source(&request(root, &fixture.url, env.clone(), control)).expect_err("canceled");

    assert_eq!(error.code, ExitCode::Canceled);
    assert!(!cache_root(&env).join(REPOS_DIRNAME).exists());
}

#[test]
fn waits_for_the_cache_lock_and_gives_up_on_cancel() {
    let dir = tempdir();
    let root = dir.path();
    let fixture = build_fixture_git_catalog(&root.join("remote")).expect("build the repository");
    let env = test_env(root);
    let held = lock_cache(&cache_root(&env), &Control::default()).expect("the first lock");
    let (flag, control) = cancelable();

    let canceler = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, Ordering::Relaxed);
    });
    let error =
        fetch_git_source(&request(root, &fixture.url, env.clone(), control)).expect_err("canceled");

    canceler.join().expect("the canceler finishes");

    assert_eq!(error.code, ExitCode::Canceled);
    assert!(
        !cache_root(&env).join(REPOS_DIRNAME).exists(),
        "nothing fetched"
    );

    drop(held);

    let fetched = fetch_git_source(&request(root, &fixture.url, env, Control::default()))
        .expect("the lock is free again");

    assert_eq!(fetched.commit, fixture.commit);
}

struct Collect(Mutex<Vec<Progress>>);

impl ProgressSink for Collect {
    fn report(&self, progress: &Progress) {
        self.0.lock().expect("unpoisoned").push(progress.clone());
    }
}

#[test]
fn reports_fetch_progress_and_still_fetches_the_same_commit() {
    let dir = tempdir();
    let root = dir.path();
    let fixture = build_fixture_git_catalog(&root.join("remote")).expect("build the repository");
    let sink = Arc::new(Collect(Mutex::default()));
    let control = Control::new(None, Some(Arc::clone(&sink) as Arc<dyn ProgressSink>));

    let fetched =
        fetch_git_source(&request(root, &fixture.url, test_env(root), control)).expect("fetch");
    let reports = sink.0.lock().expect("unpoisoned");

    assert_eq!(fetched.commit, fixture.commit);
    assert_eq!(
        reports.first().map(|progress| progress.subject.as_str()),
        Some("catalog \"company\"")
    );
    assert!(reports.len() > 1, "{reports:?}");
    assert!(
        reports
            .iter()
            .all(|progress| progress.stage == Stage::Fetching)
    );
    assert!(
        reports
            .iter()
            .skip(1)
            .all(|progress| progress.subject.starts_with("catalog \"company\": ")),
        "{reports:?}"
    );
}

#[test]
fn a_token_does_not_disturb_a_fetch_from_another_host() {
    let dir = tempdir();
    let root = dir.path();
    let fixture = build_fixture_git_catalog(&root.join("remote")).expect("build the repository");
    let env = with_github_token(&test_env(root), TOKEN);

    let fetched =
        fetch_git_source(&request(root, &fixture.url, env, Control::default())).expect("fetch");

    assert_eq!(fetched.commit, fixture.commit);
}

#[test]
fn reads_progress_lines() {
    assert_eq!(
        progress_of("Receiving objects:  45% (9/20)", "catalog \"c\""),
        Some(Progress {
            stage: Stage::Fetching,
            subject: "catalog \"c\": Receiving objects".to_owned(),
            current: 9,
            total: 20,
        })
    );
    assert_eq!(
        progress_of("remote: Counting objects: 100% (3/3), done.", "s").map(|p| p.subject),
        Some("s: Counting objects".to_owned())
    );
    assert_eq!(progress_of("fatal: nope", "s"), None);
}

#[test]
fn removes_the_program_variable_from_gits_environment() {
    let env: Env = [(GIT_PROGRAM_VAR.to_owned(), "/opt/git".to_owned())].into();

    assert!(!git_environment(&env).contains_key(GIT_PROGRAM_VAR));
}
