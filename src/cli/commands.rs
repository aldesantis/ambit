//! The CLI surface, declared in one place so usage output and dispatch cannot drift apart, and the
//! context a rule and a handler read.

use std::path::PathBuf;

use clap::builder::PossibleValuesParser;
use clap::{Args, Parser, Subcommand};
use indexmap::IndexMap;

use crate::cli::Io;
use crate::errors::{AmbitError, ExitCode, Result};
use crate::model::sources::SourceContext;
use crate::project::install::{USER_PROJECT_DIRNAME, user_project_dir};
use crate::util::env::Env;
use crate::version::VERSION;

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

/// The flags one command was given, keyed by their ids in [`Cli`]: `dryRun`, `project`, `json`,
/// `offline`, `catalog`, `capability`, … An option that was not given is absent.
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
/// A variadic argument (`[catalog]...`) and fixed arguments are flattened into the same `args`
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
/// It exists because clap's own errors cannot word every refusal in ambit's standard shape. A rule
/// reads argv only (one that touched disk or printed would be a handler), and always runs before
/// the handler, so the handler only runs on an invocation the rule accepted.
pub type CommandRule = Box<dyn Fn(&CommandContext<'_>) -> Result<()>>;

/// Handlers, keyed by command name: `"install"`. Absent means declared-but-unimplemented.
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

// The full CLI surface.
//
// Commands are flat. Project commands act on a project, and `self-update` acts on ambit itself.
// Each command declares its own flags, then `--dry-run` if it mutates, then the global flags.
//
// Nothing writes into a catalog (a catalog is Markdown and YAML in a git repo, edited directly),
// and nothing reads a catalog directory instead of an `ambit.yml`, because a catalog repo lists
// itself.
//
// The fields are never read from this type: `crate::cli::run_with` reads the parsed values back
// out of clap's matches by id into `CommandOptions`, so every handler shares one context type.
//
// The derive types carry plain comments rather than doc comments, because clap prints a doc
// comment as the command's help text.
#[derive(Debug, Parser)]
#[command(
    name = "ambit",
    version = VERSION,
    about = "a deterministic dependency manager for AI-agent capabilities"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    #[command(about = "export selected packs as Claude plugins")]
    Export {
        #[arg(
            long,
            value_name = "format",
            value_parser = ["claude-plugin"],
            allow_hyphen_values = true,
            help = "package format"
        )]
        format: Option<String>,
        #[arg(
            long,
            value_name = "dir",
            allow_hyphen_values = true,
            help = "new output directory, relative to the project"
        )]
        output: Option<String>,
        #[arg(long, help = "link skills and hook assets to local catalogs")]
        link: bool,
        #[arg(long, help = "replace an existing export directory")]
        force: bool,
        #[arg(long, help = "exit 5 when exported files or links differ")]
        check: bool,
        #[command(flatten)]
        dry_run: DryRun,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "scaffold ambit.yml, skills/, mcps/, hooks/")]
    Init {
        #[command(flatten)]
        dry_run: DryRun,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "search the merged catalog")]
    Search {
        // Required; `*` is how everything is asked for. An optional pattern would make a missing
        // argument silently mean "match everything," indistinguishable from a shell that
        // swallowed it.
        #[arg(
            value_name = "pattern",
            allow_negative_numbers = true,
            help = "glob matched against item names; `*` matches every name"
        )]
        pattern: String,
        // Repeated rather than multi-valued: a multi-valued option eats every following word, so
        // `ambit search --capability skill "foo*"` would read the pattern as a second capability.
        #[arg(
            long,
            value_name = "name",
            allow_hyphen_values = true,
            help = "limit to this catalog; repeatable"
        )]
        catalog: Vec<String>,
        #[arg(
            long,
            value_name = "kind",
            value_parser = PossibleValuesParser::new(ITEM_KIND_NAMES.iter().copied()),
            allow_hyphen_values = true,
            help = "limit to this namespace; repeatable"
        )]
        capability: Vec<String>,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "compute the bundle and print it")]
    Resolve {
        #[arg(long, help = "annotate each item with why it was selected")]
        explain: bool,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "explain why one item is in the bundle")]
    Why {
        #[arg(
            value_name = "kind:name",
            allow_negative_numbers = true,
            help = "`skill:<name>`, `mcp:<name>`, or `hook:<name>`"
        )]
        item: String,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "resolve, write lock, materialize, prune")]
    Install {
        #[arg(long, help = "fail if resolution would change ambit.lock")]
        frozen: bool,
        #[command(flatten)]
        materialize: MaterializeFlags,
        #[command(flatten)]
        audit: AuditFlags,
        #[command(flatten)]
        exec: ExecFlags,
        #[command(flatten)]
        dry_run: DryRun,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "compare what is installed against what resolve produces")]
    Status {
        #[arg(long, help = "exit 5 when drift is detected")]
        check: bool,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    // `outdated` and `update` are the only commands that reach a remote to check a ref the cache
    // already answers. `install` deliberately resolves from the cache alone and never moves a pin.
    #[command(about = "check whether any catalog's ref now names a different commit")]
    Outdated {
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "move catalog pins forward, then install")]
    Update {
        #[arg(
            value_name = "catalog",
            allow_negative_numbers = true,
            help = "catalogs to update; every one of them when none is named"
        )]
        catalogs: Vec<String>,
        #[command(flatten)]
        materialize: MaterializeFlags,
        #[command(flatten)]
        audit: AuditFlags,
        #[command(flatten)]
        exec: ExecFlags,
        #[command(flatten)]
        dry_run: DryRun,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "remove owned artifacts not in the current bundle")]
    Prune {
        #[command(flatten)]
        dry_run: DryRun,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "remove everything ambit owns")]
    Clean {
        #[command(flatten)]
        dry_run: DryRun,
        #[command(flatten)]
        globals: ProjectFlags,
    },
    // Validates everything this project configures: every catalog it lists, its own items, its
    // own `requires` entries. A catalog repo runs this too, since it lists itself as a catalog.
    #[command(about = "validate everything this project configures, for CI")]
    Validate {
        #[command(flatten)]
        globals: ProjectFlags,
    },
    #[command(about = "check preconditions, drift, ownership")]
    Doctor {
        #[command(flatten)]
        globals: ProjectFlags,
    },
    // Reads every item of every catalog the project lists, as `validate` does, not only the
    // selected ones: a catalog author runs it before publishing.
    #[command(about = "scan catalog content for hidden text and risky commands")]
    Audit {
        #[command(flatten)]
        globals: ProjectFlags,
    },
    // The only command whose subject is ambit rather than a project, so it takes no `--project`:
    // the flag would parse and do nothing. It keeps `--offline`, because a user who habitually
    // passes the flag is better served by a refusal that explains itself than by an unknown
    // argument. The version is a positional because `--version` is already how the program prints
    // its own, and a command where the two spellings meant different things would be a trap.
    #[command(about = "replace this ambit binary with a released one")]
    SelfUpdate {
        #[arg(
            value_name = "version",
            help = "release to install, like `v0.3.1`; the latest release when omitted"
        )]
        version: Option<String>,
        #[command(flatten)]
        dry_run: DryRun,
        #[command(flatten)]
        output: OutputFlags,
    },
}

// `--adopt`, `--copy` and `--link`, on the commands that install skills.
#[derive(Debug, Args)]
struct MaterializeFlags {
    #[arg(long, help = "take ownership of existing unowned artifacts")]
    adopt: bool,
    // Mutually exclusive: one copies every skill, the other symlinks every skill.
    #[arg(
        long,
        conflicts_with = "link",
        help = "copy local-source skills instead of symlinking"
    )]
    copy: bool,
    #[arg(long, help = "symlink skills instead of copying")]
    link: bool,
}

// `--no-audit`, on the commands that install. Its id is its `CommandOptions` key.
#[derive(Debug, Args)]
struct AuditFlags {
    #[arg(
        id = "noAudit",
        long = "no-audit",
        help = "skip scanning the bundle for hidden text"
    )]
    no_audit: bool,
}

// `--accept-exec`, on the commands that install. Its id is its `CommandOptions` key.
#[derive(Debug, Args)]
struct ExecFlags {
    #[arg(
        id = "acceptExec",
        long = "accept-exec",
        help = "accept hooks and stdio MCP servers not in ambit.lock"
    )]
    accept_exec: bool,
}

// `--dry-run`, only on commands that touch disk. Its id is its `CommandOptions` key.
#[derive(Debug, Args)]
struct DryRun {
    #[arg(
        id = "dryRun",
        long = "dry-run",
        help = "print the plan without touching disk"
    )]
    dry_run: bool,
}

// The global flags of every command that acts on a project.
//
// Attached to each command rather than to the program, so they are accepted after the command
// name and nowhere else.
//
// There is no `--catalog <dir>` here: every project is a catalog (it lists itself as
// `source: path:.`), so there is one subject and one directory flag. `ambit search` has its own
// `--catalog <name>` option, but that names one of several catalogs to search, not where to search
// from.
//
// `--quiet` and `--no-color` are deliberately absent: ambit has no progress chatter to suppress
// and no color to disable. Re-add either only alongside the output it would control.
#[derive(Debug, Args)]
struct ProjectFlags {
    // Its default is the cwd, which help does not print.
    #[arg(
        long,
        value_name = "dir",
        allow_hyphen_values = true,
        help = "project directory"
    )]
    project: Option<String>,
    #[arg(
        long,
        conflicts_with = "project",
        help = "act on the user-level project in ~/.ambit"
    )]
    user: bool,
    #[command(flatten)]
    output: OutputFlags,
}

// The global flags `self-update` shares with every other command.
#[derive(Debug, Args)]
struct OutputFlags {
    #[arg(long, help = "machine-readable output")]
    json: bool,
    #[arg(long, help = "use only cached catalogs")]
    offline: bool,
}

/// The `--capability` choices: [`ItemKind`](crate::model::requirement::ItemKind)'s spellings, in
/// declaration order.
pub const ITEM_KIND_NAMES: &[&str] = &["pack", "skill", "mcp", "hook"];

/// The project directory a command acts on: the user-level project under `--user`, `--project`
/// resolved against the cwd if given, otherwise the cwd.
///
/// An unknown home directory leaves `--user` naming a relative `.ambit`, as an unknown home leaves
/// the cache (`model/git.rs`); no platform ambit ships for lacks one.
pub fn project_dir_of(ctx: &CommandContext<'_>) -> PathBuf {
    if ctx.options.flag("user") {
        let dir = user_project_dir(ctx.env).unwrap_or_else(|| PathBuf::from(USER_PROJECT_DIRNAME));

        return ctx.cwd.join(dir);
    }

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
