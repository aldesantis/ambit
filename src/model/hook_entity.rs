//! Hook entity parsing.
//!
//! One shape, one place it can be written: `hooks/<name>/hook.yml` in a catalog. A project that
//! defines a hook of its own lists itself as a catalog and puts it there, so this parser has one
//! caller and no variant to reconcile.

use crate::errors::Result;
use crate::model::expectation::{Expectation, parse_expectations};
use crate::model::yaml::YamlMapping;
use crate::util::string_enum;
use crate::util::text::{is_js_whitespace, js_trim};

string_enum! {
    /// The events with a real mapping in two or more harnesses, in the order reports list them.
    ///
    /// These use Claude's `PascalCase` spellings as the neutral vocabulary. Most harnesses use
    /// them verbatim; Cursor, Gemini and Kiro map them (`harness/definitions.rs`), and a harness
    /// with no counterpart for one skips hooks on it.
    pub enum HookEvent {
        SessionStart => "SessionStart",
        UserPromptSubmit => "UserPromptSubmit",
        PreToolUse => "PreToolUse",
        PostToolUse => "PostToolUse",
        Stop => "Stop",
        SubagentStop => "SubagentStop",
        PreCompact => "PreCompact",
        SessionEnd => "SessionEnd",
    }
}

/// Every hook event, in the order reports list them.
pub const HOOK_EVENTS: &[HookEvent] = HookEvent::ALL;

/// The events a `matcher` means anything for.
pub const MATCHABLE_EVENTS: &[HookEvent] = &[HookEvent::PreToolUse, HookEvent::PostToolUse];

string_enum! {
    /// What a hook's `command` is, which decides whether ambit rewrites it and ships bytes beside
    /// it.
    ///
    /// Declared rather than derived. `guard.sh` (a shipped script) and `prettier` (a program on
    /// `PATH`) are not distinguishable by looking, so the author must say which. Guessing from
    /// whether the first token carries a `/` or a `.` was tried and got `python3.11` and
    /// `node hook.js` wrong.
    pub enum HookType {
        Command => "command",
        Script => "script",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookEntity {
    pub name: String,
    /// Carried into reports.
    pub description: Option<String>,
    pub event: HookEvent,
    /// Tool-name filter. Only ever set on one of [`MATCHABLE_EVENTS`].
    pub matcher: Option<String>,
    /// How to read [`HookEntity::command`].
    pub r#type: HookType,
    /// What the hook runs, read according to [`HookEntity::type`]: a command line the harness
    /// executes as written, or a path (relative to the hook's own directory) to a script the hook
    /// ships, followed by any arguments.
    ///
    /// `${VAR}` references are left intact, unlike an MCP transport's. A hook command runs in a
    /// shell the harness spawns, so `${VAR}` already expands correctly there, and ambit does not
    /// parse the shell fragment to translate it.
    pub command: String,
    /// Seconds. Rendered where the harness has a field for it.
    pub timeout: Option<i64>,
    /// What must be true of the world for this hook to work: what its command reads, today.
    pub expects: Vec<Expectation>,
}

/// The program a `command` runs: its first whitespace-separated token.
///
/// For a `script` hook this is the shipped file, and everything after it is arguments, a shell
/// fragment ambit does not parse and must not rewrite. So `guard.sh --strict` ships `guard.sh` and
/// passes `--strict` through untouched.
pub fn command_program(command: &str) -> String {
    js_trim(command)
        .split(is_js_whitespace)
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// The file a `script` hook's program names, as its own directory holds it.
///
/// `./guard.sh` and `guard.sh` name the same file; the leading `./` is optional under
/// `type: script` and stripped here so one spelling reaches disk. That lets the existence check
/// look for exactly what the rewrite later writes.
pub fn script_reference(program: &str) -> String {
    program.strip_prefix("./").unwrap_or(program).to_owned()
}

const ENTITY_KEYS: &[&str] = &[
    "command",
    "description",
    "event",
    "expects",
    "matcher",
    "name",
    "timeout",
    "type",
];

/// A list of spellings, as a refusal names them.
fn spelled<T: std::fmt::Display>(items: &[T]) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_event(mapping: &YamlMapping) -> Result<HookEvent> {
    let event = mapping.require_string("event")?;

    HookEvent::parse(&event).ok_or_else(|| {
        mapping.key_error(
            "event",
            &format!("unknown hook event \"{event}\""),
            vec![
                format!("supported events: {}", spelled(HOOK_EVENTS)),
                format!("replace `{event}` with one of them"),
            ],
        )
    })
}

/// A `matcher` filters on a tool name, so on an event that carries no tool it selects nothing.
/// Declaring it there is an error rather than a value quietly dropped on the way to the harness.
fn parse_matcher(mapping: &YamlMapping, event: HookEvent) -> Result<Option<String>> {
    let matcher = mapping.optional_string("matcher")?;

    if matcher.is_some() && !MATCHABLE_EVENTS.contains(&event) {
        return Err(mapping.key_error(
            "matcher",
            &format!("`matcher` is not meaningful for {event}"),
            vec![
                format!(
                    "it filters on a tool name, so it applies to: {}",
                    spelled(MATCHABLE_EVENTS)
                ),
                "remove `matcher`, or declare the hook on one of those events".to_owned(),
            ],
        ));
    }

    Ok(matcher)
}

fn parse_type(mapping: &YamlMapping) -> Result<HookType> {
    let r#type = mapping.require_string("type")?;

    HookType::parse(&r#type).ok_or_else(|| {
        mapping.key_error(
            "type",
            &format!("unknown hook type \"{type}\""),
            vec![
                "`command` runs a command line as written; `script` runs a file the hook's own directory ships".to_owned(),
                format!("replace `{type}` with one of them"),
            ],
        )
    })
}

/// Rejects a `type: script` whose `command` cannot name a file inside the hook's own directory.
///
/// Shape only. Whether the file actually exists is checked by the catalog, not this parser.
/// Refused here: an absolute path, and one climbing out through `..`; neither can be inside the
/// hook's directory under any contents. An empty `command` cannot reach this check, since
/// `require_string` already refuses it.
///
/// Such a command is not reinterpreted as a command line either: `command: /usr/bin/guard.sh` on
/// a hook meant to ship a script would install a hook pointing outside the catalog. The fix is
/// `type: command`.
fn assert_script_reference(mapping: &YamlMapping, command: &str) -> Result<()> {
    let reference = script_reference(&command_program(command));

    let problem = if reference.starts_with('/') {
        "it is an absolute path"
    } else if reference.split('/').any(|part| part == "..") {
        "it climbs out through `..`"
    } else {
        return Ok(());
    };

    Err(mapping.key_error(
        "command",
        &format!("`type: script` needs a path inside the hook, and {problem}"),
        vec![
            "a script is a file the hook's own directory ships, named relative to it — `guard.sh`, `bin/guard.sh`".to_owned(),
            "to run something else, say `type: command` instead".to_owned(),
        ],
    ))
}

/// Parses one hook entity: a whole `hooks/<name>/hook.yml` document.
///
/// # Errors
///
/// Exit 2 for any shape violation.
pub fn parse_hook_entity(mapping: &YamlMapping) -> Result<HookEntity> {
    mapping.reject_unknown_keys(ENTITY_KEYS)?;

    let name = mapping.require_string("name")?;
    let description = mapping.optional_string("description")?;
    let event = parse_event(mapping)?;
    let matcher = parse_matcher(mapping, event)?;
    let r#type = parse_type(mapping)?;
    let command = mapping.require_string("command")?;
    let timeout = mapping.optional_integer("timeout")?;

    if r#type == HookType::Script {
        assert_script_reference(mapping, &command)?;
    }

    Ok(HookEntity {
        name,
        description,
        event,
        matcher,
        r#type,
        command,
        timeout,
        expects: parse_expectations(mapping)?,
    })
}

#[cfg(test)]
mod tests;
