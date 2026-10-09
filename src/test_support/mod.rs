#[path = "../../tests/support/fixture_catalog.rs"]
pub mod fixture_catalog;

use std::path::Path;

use crate::cli::{CaptureIo, handlers, rules, run_with};
use crate::errors::ExitCode;
use crate::util::env::Env;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliResult {
    pub code: ExitCode,
    pub stdout: String,
    pub stderr: String,
}

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

pub fn tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("ambit-test-")
        .tempdir()
        .expect("create a tempdir")
}

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

#[allow(clippy::disallowed_methods)]
fn path_var() -> Option<String> {
    std::env::var("PATH").ok()
}
