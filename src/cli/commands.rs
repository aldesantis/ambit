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

/// Builds a [`CommandHandler`] from a plain function or closure.
pub fn handler(
    f: impl Fn(&mut CommandContext<'_>) -> Result<ExitCode> + 'static,
) -> CommandHandler {
    Box::new(f)
}

/// Builds a [`CommandRule`] from a plain function or closure.
pub fn rule(f: impl Fn(&CommandContext<'_>) -> Result<()> + 'static) -> CommandRule {
    Box::new(f)
}

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
///
/// Nothing writes into a catalog (a catalog is Markdown and YAML in a git repo, edited directly),
/// and nothing reads a catalog directory instead of an `ambit.yml`, because a catalog repo lists
/// itself.
pub fn command_specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            options: vec![
                OptionSpec {
                    choices: Some(&["claude-plugin"]),
                    ..option("--format <format>", "package format")
                },
                option(
                    "--output <dir>",
                    "new output directory, relative to the project",
                ),
                option("--link", "link skills and hook assets to local catalogs"),
                option("--force", "replace an existing export directory"),
                option("--check", "exit 5 when exported files or links differ"),
            ],
            mutating: true,
            ..command("export", "export selected packs as Claude plugins")
        },
        CommandSpec {
            mutating: true,
            ..command("init", "scaffold ambit.yml, skills/, mcps/, hooks/")
        },
        CommandSpec {
            // Required; `*` is how everything is asked for. An optional pattern would make a
            // missing argument silently mean "match everything," indistinguishable from a shell
            // that swallowed it.
            args: vec![ArgSpec {
                spec: "<pattern>",
                description: "glob matched against item names; `*` matches every name",
            }],
            options: vec![
                list_option(
                    "--catalog <name>",
                    "limit to this catalog; repeatable",
                    None,
                ),
                list_option(
                    "--capability <kind>",
                    "limit to this namespace; repeatable",
                    Some(ITEM_KIND_NAMES),
                ),
            ],
            ..command("search", "search the merged catalog")
        },
        CommandSpec {
            options: vec![option(
                "--explain",
                "annotate each item with why it was selected",
            )],
            ..command("resolve", "compute the bundle and print it")
        },
        CommandSpec {
            args: vec![ArgSpec {
                spec: "<kind:name>",
                description: "`skill:<name>`, `mcp:<name>`, or `hook:<name>`",
            }],
            ..command("why", "explain why one item is in the bundle")
        },
        CommandSpec {
            options: vec![
                option("--frozen", "fail if resolution would change ambit.lock"),
                option("--adopt", "take ownership of existing unowned artifacts"),
                // Mutually exclusive: one copies every skill, the other symlinks every skill. The
                // parser enforces it, and the refusal travels out of `run` as exit 2.
                OptionSpec {
                    conflicts: &["link"],
                    ..option("--copy", "copy local-source skills instead of symlinking")
                },
                option("--link", "symlink skills instead of copying"),
            ],
            mutating: true,
            ..command("install", "resolve, write lock, materialize, prune")
        },
        CommandSpec {
            options: vec![option("--check", "exit 5 when drift is detected")],
            ..command(
                "status",
                "compare what is installed against what resolve produces",
            )
        },
        // `outdated` and `update` are the only commands that reach a remote to check a ref the
        // cache already answers. `install` deliberately resolves from the cache alone and never
        // moves a pin.
        command(
            "outdated",
            "check whether any catalog's ref now names a different commit",
        ),
        CommandSpec {
            args: vec![ArgSpec {
                spec: "[catalog...]",
                description: "catalogs to update; every one of them when none is named",
            }],
            options: vec![
                option("--adopt", "take ownership of existing unowned artifacts"),
                OptionSpec {
                    conflicts: &["link"],
                    ..option("--copy", "copy local-source skills instead of symlinking")
                },
                option("--link", "symlink skills instead of copying"),
            ],
            mutating: true,
            ..command("update", "move catalog pins forward, then install")
        },
        CommandSpec {
            mutating: true,
            ..command("prune", "remove owned artifacts not in the current bundle")
        },
        CommandSpec {
            mutating: true,
            ..command("clean", "remove everything ambit owns")
        },
        // Validates everything this project configures: every catalog it lists, its own items,
        // its own `requires` entries. A catalog repo runs this too, since it lists itself as a
        // catalog.
        command(
            "validate",
            "validate everything this project configures, for CI",
        ),
        command("doctor", "check preconditions, drift, ownership"),
        // The only command whose subject is ambit rather than a project. The version is a
        // positional because `--version` is already how the program prints its own, and a command
        // where the two spellings meant different things would be a trap.
        CommandSpec {
            args: vec![ArgSpec {
                spec: "[version]",
                description: "release to install, like `v0.3.1`; the latest release when omitted",
            }],
            mutating: true,
            reads_project: false,
            ..command(
                "self-update",
                "replace this ambit binary with a released one",
            )
        },
    ]
}

/// The `--capability` choices: [`ItemKind`](crate::model::requirement::ItemKind)'s spellings, in
/// declaration order.
pub const ITEM_KIND_NAMES: &[&str] = &["pack", "skill", "mcp", "hook"];

/// A command acting on a project, with no arguments or options of its own.
fn command(name: &'static str, summary: &'static str) -> CommandSpec {
    CommandSpec {
        name,
        summary,
        args: Vec::new(),
        options: Vec::new(),
        mutating: false,
        reads_project: true,
        subcommands: None,
    }
}

/// A plain option: no choices, given at most once (a repeat overwrites), no conflicts.
pub const fn option(flags: &'static str, description: &'static str) -> OptionSpec {
    OptionSpec {
        flags,
        description,
        choices: None,
        repeatable: false,
        conflicts: &[],
    }
}

/// A flag that may be given more than once, collecting into a list: `--catalog a --catalog b`.
///
/// `allowed`, when given, is checked on every value and listed in help as the choices.
pub const fn list_option(
    flags: &'static str,
    description: &'static str,
    allowed: Option<&'static [&'static str]>,
) -> OptionSpec {
    OptionSpec {
        flags,
        description,
        choices: allowed,
        repeatable: true,
        conflicts: &[],
    }
}

/// `--dry-run`, added only to commands that touch disk.
pub(crate) const DRY_RUN: OptionSpec = option("--dry-run", "print the plan without touching disk");

/// Flags every acting command accepts, after its own and `--dry-run`.
///
/// Attached to each command rather than to the program, because program-level options are only
/// accepted before the command name; without this, `ambit install --json` would not parse.
///
/// There is no `--catalog <dir>` here: every project is a catalog now (it lists itself as
/// `source: path:.`), so there is one subject and one directory flag. `ambit search` has its own
/// `--catalog <name>` option, but that names one of several catalogs to search, not where to
/// search from.
///
/// `--quiet` and `--no-color` are deliberately absent: ambit has no progress chatter to suppress
/// and no color to disable, so both flags used to parse and do nothing. Re-add either only
/// alongside the output it would control.
///
/// The same rule is why `--project` is conditional. `self-update`'s subject is the binary, not a
/// project, so the flag would parse and do nothing there; [`CommandSpec::reads_project`] is how a
/// command opts out. `--offline` stays on it, because a user who habitually passes the flag is
/// better served by a refusal that explains itself than by `unknown option`.
pub(crate) fn global_options(reads_project: bool) -> Vec<OptionSpec> {
    let mut options = Vec::with_capacity(3);

    if reads_project {
        // Its default is the cwd, which help does not print.
        options.push(option("--project <dir>", "project directory"));
    }

    options.push(option("--json", "machine-readable output"));
    options.push(option("--offline", "use only cached catalogs"));
    options
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
