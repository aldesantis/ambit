//! Environment variables in harness configs.
//!
//! A catalog writes `${VAR}` in an MCP entity's headers, arguments and env map, and names in
//! `expects` the variables a server needs. ambit resolves neither into the config file it writes.
//! It translates them into the reference syntax the target harness expands at spawn time (`${VAR}`
//! for Claude Code, Codex, Gemini, Grok and Kiro, `${env:VAR}` for Copilot, Cursor and Devin,
//! `{env:VAR}` for opencode) and leaves the value in the environment.
//!
//! Writing the resolved value would put a live credential into `.mcp.json`, a file ambit does not
//! gitignore because teams legitimately commit it. Writing a reference instead keeps the installed
//! config identical on every machine, so two people resolving the same bundle get byte-identical
//! files and `ambit status` never reports drift because of a token difference.
//!
//! This means ambit cannot tell from the file whether a variable is set. `ambit doctor` checks the
//! environment directly instead.

use std::sync::LazyLock;

use indexmap::{IndexMap, IndexSet};
use regex::Regex;

use crate::util::cmp::js_cmp;

/// A `${VAR}` reference as a catalog writes it.
///
/// Anchored to the shell-variable character set, so a `${...}` in some other syntax is left alone.
static ENV_PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("ENV_PLACEHOLDER is a valid pattern")
});

/// [`ENV_PLACEHOLDER`], anchored to the whole value.
static SOLE_PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\$\{([A-Za-z_][A-Za-z0-9_]*)\}$").expect("SOLE_PLACEHOLDER is a valid pattern")
});

/// How one harness spells a reference to an environment variable in its own config.
pub type EnvRefStyle = fn(&str) -> String;

/// Claude Code, Codex, Gemini, Grok and Kiro: plain shell syntax.
pub fn shell_ref(name: &str) -> String {
    format!("${{{name}}}")
}

/// Copilot, Cursor and Devin.
pub fn namespaced_ref(name: &str) -> String {
    format!("${{env:{name}}}")
}

/// opencode.
pub fn braced_ref(name: &str) -> String {
    format!("{{env:{name}}}")
}

/// Rewrites every `${VAR}` in a value into the harness's own reference syntax.
///
/// A no-op for harnesses whose syntax already is `${VAR}`.
pub fn translate_refs(value: &str, style: EnvRefStyle) -> String {
    ENV_PLACEHOLDER
        .replace_all(value, |captures: &regex::Captures<'_>| style(&captures[1]))
        .into_owned()
}

/// Every variable a value references, in first-appearance order.
pub fn referenced_names(value: &str) -> Vec<String> {
    ENV_PLACEHOLDER
        .captures_iter(value)
        .map(|captures| captures[1].to_owned())
        .collect()
}

/// The variable a value is entirely one reference to, if it is.
///
/// Codex takes a header whose value is a bare variable reference as `env_http_headers`, naming the
/// variable rather than embedding it. This distinguishes a header it can express that way from one
/// that has to be written literally.
pub fn sole_reference(value: &str) -> Option<String> {
    SOLE_PLACEHOLDER
        .captures(value)
        .map(|captures| captures[1].to_owned())
}

/// The env map a stdio server is given, so the harness passes the variables through to the
/// process.
///
/// Two sources. Every name in `expected` is passed under its own name, which is the whole of it for
/// a server whose variables are called what the machine calls them. An entry in `declared` names
/// the variable the process reads and supplies its value, so a server can be given a name nothing
/// in the environment has.
///
/// A variable a `declared` value references is not also passed under its own name: the entry says
/// where that variable goes, and passing it through as well would hand the process a second name
/// its author did not ask for. `declared` also wins on a key collision, for the same reason.
///
/// Sorted by name, so the installed file does not churn when a catalog reorders its own keys.
/// `None` when both sources are empty, so a caller can leave the key out entirely.
pub fn stdio_env(
    expected: &[String],
    declared: &IndexMap<String, String>,
    style: EnvRefStyle,
) -> Option<IndexMap<String, String>> {
    let supplies: IndexSet<String> = declared
        .values()
        .flat_map(|value| referenced_names(value))
        .collect();
    let mut env: IndexMap<String, String> = IndexMap::new();

    for name in expected {
        if !supplies.contains(name) {
            env.insert(name.clone(), style(name));
        }
    }

    for (name, value) in declared {
        env.insert(name.clone(), translate_refs(value, style));
    }

    if env.is_empty() {
        return None;
    }

    env.sort_by(|a, _, b, _| js_cmp(a, b));
    Some(env)
}

#[cfg(test)]
mod tests;
