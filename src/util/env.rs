//! The process environment, as a value passed down from `main`.
//!
//! Nothing below `main` reads the real environment: a command run sees the one snapshot it was
//! given, and a test builds its own map instead of mutating process state other test threads share.

use std::collections::BTreeMap;

/// Environment variables by name.
pub type Env = BTreeMap<String, String>;

/// The real process environment. Called from `main` only; variables that are not valid UTF-8
/// are skipped.
pub fn snapshot() -> Env {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}
