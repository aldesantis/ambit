//! The program: parsing argv against the declared surface, dispatching to a handler, and turning
//! every outcome into an exit code.

pub mod commands;
pub mod handlers;
pub mod output;

#[cfg(test)]
mod tests;

use std::io::{IsTerminal as _, Write as _};
use std::path::Path;

use clap::{ArgAction, ArgMatches, CommandFactory as _};
use indexmap::IndexMap;

use crate::cli::commands::{
    Cli, CommandContext, CommandHandlers, CommandOptions, CommandRules, OptionValue, handler,
    not_implemented, rule,
};
use crate::cli::handlers::outdated::refuses_offline_rule;
use crate::cli::handlers::self_update::refuses_offline_self_update_rule;
use crate::cli::handlers::{
    audit, clean, doctor, export, init, install, outdated, prune, resolve, search, self_update,
    status, update, validate, why,
};
use crate::errors::{AmbitError, ExitCode};
use crate::util::env::Env;

/// The widest help wraps to, and its width when stdout is not a terminal: clap's own defaults,
/// fixed here so clap never reads the terminal or `COLUMNS` itself.
pub const MAX_HELP_WIDTH: usize = 100;

/// Where a command's output goes, one line at a time.
pub trait Io {
    fn stdout(&mut self, line: &str);
    fn stderr(&mut self, line: &str);

    /// The terminal's width when stdout is one. `None` means [`MAX_HELP_WIDTH`].
    fn help_width(&self) -> Option<usize> {
        None
    }
}

/// The real streams. Each line is written with a trailing newline; a closed pipe is ignored, so
/// `ambit search '*' | head` ends quietly.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdIo;

impl Io for StdIo {
    fn stdout(&mut self, line: &str) {
        let _ = writeln!(std::io::stdout().lock(), "{line}");
    }

    fn stderr(&mut self, line: &str) {
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }

    fn help_width(&self) -> Option<usize> {
        let stream = std::io::stdout();

        stream
            .is_terminal()
            .then(|| terminal_size::terminal_size_of(stream))
            .flatten()
            .map(|(terminal_size::Width(columns), _)| usize::from(columns))
    }
}

/// Output captured line by line, for tests.
#[cfg(test)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureIo {
    pub out: Vec<String>,
    pub err: Vec<String>,
}

#[cfg(test)]
impl Io for CaptureIo {
    fn stdout(&mut self, line: &str) {
        self.out.push(line.to_owned());
    }

    fn stderr(&mut self, line: &str) {
        self.err.push(line.to_owned());
    }
}

/// Handlers, keyed by command name. Every command the surface declares has one here; a command
/// added without an entry reports itself unimplemented (exit 1) rather than silently succeeding.
pub fn handlers() -> CommandHandlers {
    CommandHandlers::from_iter([
        ("audit".to_owned(), handler(audit::audit_handler)),
        ("export".to_owned(), handler(export::export_handler)),
        ("clean".to_owned(), handler(clean::clean_handler)),
        ("doctor".to_owned(), handler(doctor::doctor_handler)),
        ("init".to_owned(), handler(init::init_handler)),
        ("install".to_owned(), handler(install::install_handler)),
        ("outdated".to_owned(), handler(outdated::outdated_handler)),
        ("prune".to_owned(), handler(prune::prune_handler)),
        ("resolve".to_owned(), handler(resolve::resolve_handler)),
        ("search".to_owned(), handler(search::search_handler)),
        (
            "self-update".to_owned(),
            handler(self_update::self_update_handler),
        ),
        ("status".to_owned(), handler(status::status_handler)),
        ("update".to_owned(), handler(update::update_handler)),
        ("validate".to_owned(), handler(validate::validate_handler)),
        ("why".to_owned(), handler(why::why_handler)),
    ])
}

/// Flag rules, keyed by command name as [`handlers`] is: what each command refuses about the flags
/// it was given, before dispatch.
///
/// Three commands need one, and all three refuse `--offline`. `outdated` and `update` share a rule,
/// since both refuse for the same reason: only a remote knows where a ref points now. `self-update`
/// refuses for a different reason (no cache holds a binary it has not downloaded), so it carries
/// its own wording. Rules exist instead of a clap-level conflict because that produces a message
/// that names no file and gives no next step. `install`'s `--copy`/`--link` still uses a declared
/// conflict, since clap's wording for two flags that cannot appear together already says
/// everything needed.
pub fn rules() -> CommandRules {
    CommandRules::from_iter([
        ("outdated".to_owned(), rule(refuses_offline_rule)),
        (
            "self-update".to_owned(),
            rule(refuses_offline_self_update_rule),
        ),
        ("update".to_owned(), rule(refuses_offline_rule)),
    ])
}

/// Runs the CLI with the shipped handlers and rules, and returns the process exit code.
pub fn run(argv: &[String], cwd: &Path, env: &Env, io: &mut dyn Io) -> ExitCode {
    run_with(argv, cwd, env, io, &handlers(), &rules())
}

/// Runs the CLI and returns the process exit code. Never fails: every failure path is translated
/// into an exit code, with the message already printed.
///
/// Usage errors print clap's message to stderr and exit 2. `--help`, `--version` and bare `ambit`
/// print to stdout and exit 0. An [`AmbitError`] prints its `format()` and exits with its code.
pub fn run_with(
    argv: &[String],
    cwd: &Path,
    env: &Env,
    io: &mut dyn Io,
    handlers: &CommandHandlers,
    rules: &CommandRules,
) -> ExitCode {
    let width = io
        .help_width()
        .map_or(MAX_HELP_WIDTH, |width| width.min(MAX_HELP_WIDTH));
    let mut program = Cli::command().term_width(width);

    // Bare `ambit` is a request for usage, not a mistake.
    if argv.is_empty() {
        io.stdout(program.render_help().to_string().trim_end());

        return ExitCode::Success;
    }

    let words = std::iter::once("ambit").chain(argv.iter().map(String::as_str));
    let matches = match program.try_get_matches_from_mut(words) {
        Ok(matches) => matches,
        Err(error) => {
            let text = error.render().to_string();

            if error.use_stderr() {
                io.stderr(text.trim_end());
            } else {
                io.stdout(text.trim_end());
            }

            return if error.exit_code() == 0 {
                ExitCode::Success
            } else {
                ExitCode::Config
            };
        }
    };
    let (key, options, args) = invocation(&program, &matches);
    let mut ctx = CommandContext {
        options,
        args,
        cwd: cwd.to_path_buf(),
        env,
        io,
    };

    match dispatch(&key, &mut ctx, handlers, rules) {
        Ok(code) => code,
        Err(error) => {
            ctx.io.stderr(&error.format());

            error.code
        }
    }
}

/// The name, flags and positionals of the command `matches` reached.
///
/// Read back by each argument's id and action, so a flag added to [`Cli`] reaches handlers with no
/// change here. Positionals are flattened in declaration order. The surface is flat, so the
/// command is one level down.
fn invocation(
    program: &clap::Command,
    matches: &ArgMatches,
) -> (String, CommandOptions, Vec<String>) {
    let (name, matches) = matches
        .subcommand()
        .expect("the program requires a command");
    let command = program
        .find_subcommand(name)
        .expect("clap matched a declared command");

    let mut options = IndexMap::new();
    let mut args = Vec::new();

    for arg in command.get_arguments() {
        let id = arg.get_id().as_str();

        if arg.is_positional() {
            args.extend(
                matches
                    .get_many::<String>(id)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            continue;
        }

        let value = match arg.get_action() {
            ArgAction::SetTrue => matches.get_flag(id).then_some(OptionValue::Flag),
            ArgAction::Set => matches
                .get_one::<String>(id)
                .map(|value| OptionValue::Value(value.clone())),
            ArgAction::Append => matches
                .get_many::<String>(id)
                .map(|values| OptionValue::List(values.cloned().collect())),
            // `--help` and `--version`, which end the parse before this.
            _ => None,
        };

        if let Some(value) = value {
            options.insert(id.to_owned(), value);
        }
    }

    (name.to_owned(), CommandOptions(options), args)
}

/// Runs the command's rule, then its handler.
fn dispatch(
    key: &str,
    ctx: &mut CommandContext<'_>,
    handlers: &CommandHandlers,
    rules: &CommandRules,
) -> Result<ExitCode, AmbitError> {
    if let Some(rule) = rules.get(key) {
        rule(ctx)?;
    }

    let handler = handlers.get(key).ok_or_else(|| not_implemented(key))?;

    handler(ctx)
}
