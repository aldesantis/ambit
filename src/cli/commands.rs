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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptionValue {
    Flag,
    Value(String),
    List(Vec<String>),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandOptions(pub IndexMap<String, OptionValue>);

impl CommandOptions {
    pub fn flag(&self, key: &str) -> bool {
        matches!(self.0.get(key), Some(OptionValue::Flag))
    }

    pub fn value(&self, key: &str) -> Option<&str> {
        match self.0.get(key) {
            Some(OptionValue::Value(value)) => Some(value),
            _ => None,
        }
    }

    pub fn list(&self, key: &str) -> &[String] {
        match self.0.get(key) {
            Some(OptionValue::List(values)) => values,
            _ => &[],
        }
    }
}

pub struct CommandContext<'a> {
    pub options: CommandOptions,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: &'a Env,
    pub io: &'a mut dyn Io,
}

pub type CommandHandler = Box<dyn Fn(&mut CommandContext<'_>) -> Result<ExitCode>>;

pub type CommandRule = Box<dyn Fn(&CommandContext<'_>) -> Result<()>>;

pub type CommandHandlers = IndexMap<String, CommandHandler>;

pub type CommandRules = IndexMap<String, CommandRule>;

pub fn handler(
    f: impl Fn(&mut CommandContext<'_>) -> Result<ExitCode> + 'static,
) -> CommandHandler {
    Box::new(f)
}

pub fn rule(f: impl Fn(&CommandContext<'_>) -> Result<()> + 'static) -> CommandRule {
    Box::new(f)
}

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
        #[arg(
            value_name = "pattern",
            allow_negative_numbers = true,
            help = "glob matched against item names; `*` matches every name"
        )]
        pattern: String,
        // Repeated, not multi-valued: a multi-valued option would swallow the following pattern.
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
    #[command(about = "scan catalog content for hidden text and risky commands")]
    Audit {
        #[command(flatten)]
        globals: ProjectFlags,
    },
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

#[derive(Debug, Args)]
struct MaterializeFlags {
    #[arg(long, help = "take ownership of existing unowned artifacts")]
    adopt: bool,
    #[arg(
        long,
        conflicts_with = "link",
        help = "copy local-source skills instead of symlinking"
    )]
    copy: bool,
    #[arg(long, help = "symlink skills instead of copying")]
    link: bool,
}

#[derive(Debug, Args)]
struct AuditFlags {
    #[arg(
        id = "noAudit",
        long = "no-audit",
        help = "skip scanning the bundle for hidden text"
    )]
    no_audit: bool,
}

#[derive(Debug, Args)]
struct ExecFlags {
    #[arg(
        id = "acceptExec",
        long = "accept-exec",
        help = "accept hooks and stdio MCP servers not in ambit.lock"
    )]
    accept_exec: bool,
}

#[derive(Debug, Args)]
struct DryRun {
    #[arg(
        id = "dryRun",
        long = "dry-run",
        help = "print the plan without touching disk"
    )]
    dry_run: bool,
}

#[derive(Debug, Args)]
struct ProjectFlags {
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

#[derive(Debug, Args)]
struct OutputFlags {
    #[arg(long, help = "machine-readable output")]
    json: bool,
    #[arg(long, help = "use only cached catalogs")]
    offline: bool,
}

pub const ITEM_KIND_NAMES: &[&str] = &["pack", "skill", "mcp", "hook"];

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

pub fn json_requested(ctx: &CommandContext<'_>) -> bool {
    ctx.options.flag("json")
}

pub fn offline_requested(ctx: &CommandContext<'_>) -> bool {
    ctx.options.flag("offline")
}

pub fn dry_run_requested(ctx: &CommandContext<'_>) -> bool {
    ctx.options.flag("dryRun")
}

pub fn list_of<'c>(ctx: &'c CommandContext<'_>, name: &str) -> &'c [String] {
    ctx.options.list(name)
}

pub fn source_context_of(ctx: &CommandContext<'_>) -> SourceContext {
    SourceContext {
        project_dir: project_dir_of(ctx),
        env: ctx.env.clone(),
        offline: offline_requested(ctx),
    }
}

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
