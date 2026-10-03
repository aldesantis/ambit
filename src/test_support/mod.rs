//! Helpers for the in-crate tests: the fixture catalog, running the CLI in-process, and
//! disposable projects.
//!
//! The environment is always built here and passed in, never read from or written to the process,
//! because cargo runs tests on parallel threads of one process.

#[path = "../../tests/support/fixture_catalog.rs"]
pub mod fixture_catalog;

use std::path::Path;

use crate::cli::{CaptureIo, handlers, rules, run_with};
use crate::errors::ExitCode;
use crate::util::env::Env;

/// What one in-process CLI run produced. `stdout` and `stderr` hold every line followed by `\n`,
/// exactly as the real process would have written them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliResult {
    pub code: ExitCode,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `ambit <args>` in-process with the shipped handlers, in `cwd`, with exactly `env`.
pub fn run_cli(args: &[&str], cwd: &Path, env: &Env) -> CliResult {
    let argv: Vec<String> = args.iter().map(|&arg| arg.to_owned()).collect();
    let mut io = CaptureIo::default();
    let code = run_with(&argv, cwd, env, &mut io, &handlers(), &rules());
    let joined = |lines: &[String]| lines.iter().map(|line| line.clone() + "\n").collect();

    CliResult {
        code,
        stdout: joined(&io.out),
        stderr: joined(&io.err),
    }
}

/// A fresh temporary directory, removed when dropped.
pub fn tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("ambit-test-")
        .tempdir()
        .expect("create a tempdir")
}

/// A minimal environment for a test: the real `PATH` (so `git` is found), `HOME` and
/// `XDG_CACHE_HOME` under `root` (so nothing touches the real cache), and the update check off.
pub fn test_env(root: &Path) -> Env {
    let mut env = Env::new();

    if let Some(path) = path_var() {
        env.insert("PATH".to_owned(), path);
    }

    env.insert(
        "HOME".to_owned(),
        root.join("home").to_string_lossy().into_owned(),
    );
    env.insert(
        "XDG_CACHE_HOME".to_owned(),
        root.join("cache").to_string_lossy().into_owned(),
    );
    env.insert("AMBIT_NO_UPDATE_CHECK".to_owned(), "1".to_owned());
    env
}

/// The real `PATH`, read once here so tests can find `git`. Reading is safe in parallel; only
/// writing the process environment is forbidden.
#[allow(clippy::disallowed_methods)]
fn path_var() -> Option<String> {
    std::env::var("PATH").ok()
}
