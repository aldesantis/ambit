//! The process environment, as a value passed down from `main`.
//!
//! Nothing below `main` reads the real environment: a command run sees the one snapshot it was
//! given, and a test builds its own map instead of mutating process state other test threads share.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Environment variables by name.
pub type Env = BTreeMap<String, String>;

/// The real process environment. Called from `main` only; variables that are not valid UTF-8
/// are skipped.
pub fn snapshot() -> Env {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// The home directory: `HOME` from `env`, or the platform's own answer when `env` has none.
///
/// `HOME` wins so a test can point a home directory somewhere disposable.
pub fn home_dir(env: &Env) -> Option<PathBuf> {
    match env.get("HOME") {
        Some(home) => Some(PathBuf::from(home)),
        #[allow(deprecated)]
        None => std::env::home_dir(),
    }
}
