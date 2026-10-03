//! Hook entity parsing.
//!
//! One shape, one place it can be written: `hooks/<name>/hook.yml` in a catalog. A project that
//! defines a hook of its own lists itself as a catalog and puts it there, so this parser has one
//! caller and no variant to reconcile.

use crate::errors::Result;
use crate::model::expectation::Expectation;
use crate::model::yaml::YamlMapping;
use crate::util::string_enum;

string_enum! {
    /// The events with a real mapping in two or more harnesses, in the order reports list them.
    ///
    /// These use Claude's `PascalCase` spellings as the neutral vocabulary. Codex and VS Code use
    /// them verbatim, so only Cursor needs a mapping; a new, fourth spelling would mean every
    /// harness needs one.
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

/// Every hook type, in declaration order.
pub const HOOK_TYPES: &[HookType] = HookType::ALL;

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
    let _ = command;
    todo!("port model/hook-entity.ts:commandProgram")
}

/// The file a `script` hook's program names, as its own directory holds it.
///
/// `./guard.sh` and `guard.sh` name the same file; the leading `./` is optional under
/// `type: script` and stripped here so one spelling reaches disk. That lets the existence check
/// look for exactly what the rewrite later writes.
pub fn script_reference(program: &str) -> String {
    let _ = program;
    todo!("port model/hook-entity.ts:scriptReference")
}

/// Parses one hook entity: a whole `hooks/<name>/hook.yml` document.
///
/// # Errors
///
/// Exit 2 for any shape violation.
pub fn parse_hook_entity(mapping: &YamlMapping) -> Result<HookEntity> {
    let _ = mapping;
    todo!("port model/hook-entity.ts:parseHookEntity")
}
