//! Environment variables in harness configs.
//!
//! A catalog writes `${VAR}` in an MCP entity's headers, arguments and env map, and names in
//! `expects` the variables a server needs. ambit resolves neither into the config file it writes.
//! It translates them into the reference syntax the target harness expands at spawn time (`${VAR}`
//! for Claude Code and Codex, `${env:VAR}` for Cursor and VS Code, `{env:VAR}` for opencode) and
//! leaves the value in the environment.
//!
//! Writing the resolved value would put a live credential into `.mcp.json`, a file ambit does not
//! gitignore because teams legitimately commit it. Writing a reference instead keeps the installed
//! config identical on every machine, so two people resolving the same bundle get byte-identical
//! files and `ambit status` never reports drift because of a token difference.
//!
//! This means ambit cannot tell from the file whether a variable is set. `ambit doctor` checks the
//! environment directly instead.

use indexmap::IndexMap;

/// How one harness spells a reference to an environment variable in its own config.
pub type EnvRefStyle = fn(&str) -> String;

/// Claude Code and Codex: plain shell syntax.
pub fn shell_ref(name: &str) -> String {
    format!("${{{name}}}")
}

/// Cursor and VS Code.
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
    let _ = (value, style);
    todo!("port harness/env.ts:translateRefs")
}

/// Every variable a value references, in first-appearance order.
pub fn referenced_names(value: &str) -> Vec<String> {
    let _ = value;
    todo!("port harness/env.ts:referencedNames")
}

/// The variable a value is entirely one reference to, if it is.
///
/// Codex takes a header whose value is a bare variable reference as `env_http_headers`, naming the
/// variable rather than embedding it. This distinguishes a header it can express that way from one
/// that has to be written literally.
pub fn sole_reference(value: &str) -> Option<String> {
    let _ = value;
    todo!("port harness/env.ts:soleReference")
}

/// The env map a stdio server is given, so the harness passes the variables through to the
/// process.
///
/// Every name in `expected` is passed under its own name. An entry in `declared` names the
/// variable the process reads and supplies its value. A variable a `declared` value references is
/// not also passed under its own name, and `declared` wins on a key collision.
///
/// Sorted by name, so the installed file does not churn when a catalog reorders its own keys.
/// `None` when both sources are empty, so a caller can leave the key out entirely.
pub fn stdio_env(
    expected: &[String],
    declared: &IndexMap<String, String>,
    style: EnvRefStyle,
) -> Option<IndexMap<String, String>> {
    let _ = (expected, declared, style);
    todo!("port harness/env.ts:stdioEnv")
}
