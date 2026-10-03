//! The CLI surface, declared in one place so usage output and dispatch cannot drift apart, and the
//! context a rule and a handler read.

use std::path::PathBuf;

use indexmap::IndexMap;

use crate::cli::Io;
use crate::errors::{AmbitError, ExitCode, Result};
use crate::model::sources::SourceContext;
use crate::util::env::Env;

/// What one parsed option holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionValue {
    /// A boolean flag that was given.
    Flag,
    /// A single-valued option.
    Value(String),
    /// A repeatable option, every value in the order typed.
    List(Vec<String>),
}

/// The flags one command was given, keyed by commander's camelCase attribute names: `dryRun`,
/// `project`, `json`, `offline`, `catalog`, `capability`, … An option that was not given is absent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandOptions(pub IndexMap<String, OptionValue>);

impl CommandOptions {
    /// Whether the boolean flag `key` was given.
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.0.get(key), Some(OptionValue::Flag))
    }

    /// The value of the single-valued option `key`, if given.
    pub fn value(&self, key: &str) -> Option<&str> {
        match self.0.get(key) {
            Some(OptionValue::Value(value)) => Some(value),
            _ => None,
        }
    }

    /// Every value given for the repeatable option `key`, in the order typed. Empty when the flag
    /// was never given; there is no separate way to spell "given but empty".
    pub fn list(&self, key: &str) -> &[String] {
        match self.0.get(key) {
            Some(OptionValue::List(values)) => values,
            _ => &[],
        }
    }
}

/// What a rule and a handler both read: the flags and positionals parsed for one command, and the
/// process boundary (cwd, environment, output) passed in rather than reached for.
///
/// A variadic argument (`[catalog...]`) and fixed arguments are flattened into the same `args`
/// list. No command mixes the two, so the flattening is never ambiguous.
pub struct CommandContext<'a> {
    pub options: CommandOptions,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: &'a Env,
    pub io: &'a mut dyn Io,
}

/// Runs one command and returns its exit code. A meaningful non-zero code (drift, doctor failures)
/// travels out as `Ok` without being dressed up as an error.
pub type CommandHandler = Box<dyn Fn(&mut CommandContext<'_>) -> Result<ExitCode>>;

/// A rule about the flags one command was given, enforced after parsing and before the handler.
///
/// It exists because commander's own primitives cannot word every refusal in ambit's standard
/// shape. A rule reads argv only (one that touched disk or printed would be a handler), and always
/// runs before the handler, so the handler only runs on an invocation the rule accepted.
pub type CommandRule = Box<dyn Fn(&CommandContext<'_>) -> Result<()>>;

/// Handlers, keyed by the words a user types: `"install"`, or `"<group> <command>"` for a nested
/// one. Absent means declared-but-unimplemented.
pub type CommandHandlers = IndexMap<String, CommandHandler>;

/// Flag rules, keyed exactly as [`CommandHandlers`] is. Absent means the command's flags need no
/// rule beyond what the declaration already states.
pub type CommandRules = IndexMap<String, CommandRule>;

/// One positional argument in commander syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArgSpec {
    /// `<pattern>`, `[catalog...]`.
    pub spec: &'static str,
    pub description: &'static str,
}

/// One command-specific option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OptionSpec {
    /// `--format <format>`, `--json`.
    pub flags: &'static str,
    pub description: &'static str,
    /// The values the option accepts, rendered in help as ` (choices: "a", "b")`.
    pub choices: Option<&'static [&'static str]>,
    /// Whether the option may be given more than once, collecting into a list.
    ///
    /// Repeating rather than accepting a list is deliberate: a variadic option eats every following
    /// word until the next `-`, so `ambit search --capability skill "foo*"` would read the pattern
    /// as a second capability.
    pub repeatable: bool,
    /// Attribute names of options this one cannot be given with (`link` for `--copy`).
    pub conflicts: &'static [&'static str],
}

/// One command, or a group of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub summary: &'static str,
    /// Positional arguments, in the order they are given.
    pub args: Vec<ArgSpec>,
    /// Command-specific flags, on top of the global set.
    pub options: Vec<OptionSpec>,
    /// Whether this command mutates its subject, and so takes `--dry-run`.
    pub mutating: bool,
    /// Whether this command acts on a project, and so takes `--project`. Only `self-update` is
    /// false: its subject is the ambit binary.
    pub reads_project: bool,
    /// Nested commands, for a name that is a group rather than a command.
    ///
    /// A group has no action of its own: bare `ambit <group>` prints its usage. Nothing declares
    /// one today; the group code path stays as the seam a future group would need, exercised
    /// directly by the catalog tests.
    pub subcommands: Option<Vec<CommandSpec>>,
}

/// The full CLI surface.
///
/// Commands are flat. Project commands act on a project, and `self-update` acts on ambit itself.
/// The global flags (`--project`, `--json`, `--offline`) and `--dry-run` are added by the parser
/// from `reads_project` and `mutating`, after the command's own options, in that order.
pub fn command_specs() -> Vec<CommandSpec> {
    todo!("port cli/commands.ts:COMMAND_SPECS")
}

/// The project directory a command acts on: `--project` resolved against the cwd if given,
/// otherwise the cwd.
pub fn project_dir_of(ctx: &CommandContext<'_>) -> PathBuf {
    match ctx.options.value("project") {
        Some(given) => crate::util::path::resolve(&ctx.cwd, given),
        None => ctx.cwd.clone(),
    }
}

/// Whether `--json` was requested.
pub fn json_requested(ctx: &CommandContext<'_>) -> bool {
    ctx.options.flag("json")
}

/// Whether `--offline` was requested: resolve from the cache alone.
pub fn offline_requested(ctx: &CommandContext<'_>) -> bool {
    ctx.options.flag("offline")
}

/// Whether `--dry-run` was requested: report what the command would do and touch nothing.
pub fn dry_run_requested(ctx: &CommandContext<'_>) -> bool {
    ctx.options.flag("dryRun")
}

/// Every value given for a repeatable flag, in the order they were typed.
pub fn list_of<'c>(ctx: &'c CommandContext<'_>, name: &str) -> &'c [String] {
    ctx.options.list(name)
}

/// What resolving a `source` needs from a command: the project directory, the environment the
/// catalog cache is looked for in, and whether fetching is allowed at all.
pub fn source_context_of(ctx: &CommandContext<'_>) -> SourceContext {
    SourceContext {
        project_dir: project_dir_of(ctx),
        env: ctx.env.clone(),
        offline: offline_requested(ctx),
    }
}

/// The error for a command declared in the surface with no handler on this build.
pub fn not_implemented(name: &str) -> AmbitError {
    AmbitError::new(
        ExitCode::Internal,
        format!("command \"{name}\" is not implemented yet"),
        [
            "it is declared in the CLI surface but has no behaviour on this build",
            "run `ambit --help` to see what does work",
        ],
    )
}
