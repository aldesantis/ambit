use crate::errors::Result;
use crate::model::expectation::{Expectation, parse_expectations};
use crate::model::yaml::YamlMapping;
use crate::util::string_enum;
use crate::util::text::{is_js_whitespace, js_trim};

string_enum! {
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

pub const HOOK_EVENTS: &[HookEvent] = HookEvent::ALL;

pub const MATCHABLE_EVENTS: &[HookEvent] = &[HookEvent::PreToolUse, HookEvent::PostToolUse];

string_enum! {
    /// Declared rather than inferred: guessing from a `/` or `.` in the first token misreads
    /// `python3.11` and `node hook.js`.
    pub enum HookType {
        Command => "command",
        Script => "script",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookEntity {
    pub name: String,
    pub description: Option<String>,
    pub event: HookEvent,
    pub matcher: Option<String>,
    pub r#type: HookType,
    /// `${VAR}` is left intact, unlike in MCP transports: the harness's shell expands it.
    pub command: String,
    pub timeout: Option<i64>,
    pub expects: Vec<Expectation>,
}

pub fn command_program(command: &str) -> String {
    js_trim(command)
        .split(is_js_whitespace)
        .next()
        .unwrap_or_default()
        .to_owned()
}

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
