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

/// The value of `name`, treating an empty value as unset, as `process.env.X || …` did.
pub fn non_empty<'e>(env: &'e Env, name: &str) -> Option<&'e str> {
    env.get(name)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treats_empty_values_as_unset() {
        let env: Env = [
            ("A".to_owned(), String::new()),
            ("B".to_owned(), "x".to_owned()),
        ]
        .into();

        assert_eq!(non_empty(&env, "A"), None);
        assert_eq!(non_empty(&env, "B"), Some("x"));
        assert_eq!(non_empty(&env, "C"), None);
    }
}
