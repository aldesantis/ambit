//! The program: parsing argv against the declared surface, dispatching to a handler, and turning
//! every outcome into an exit code.
//!
//! The parser (`parser.rs`) and help renderer (`help.rs`) are a hand port of the subset of
//! commander 15 the TypeScript build used, so usage output and error wording stay byte-identical.

pub mod commands;
pub mod handlers;
pub mod help;
pub mod output;
pub mod parser;
pub mod suggest;

#[cfg(test)]
mod tests;

use std::io::{IsTerminal as _, Write as _};
use std::path::Path;

use crate::cli::commands::{
    CommandContext, CommandHandlers, CommandRules, CommandSpec, command_specs, handler,
    not_implemented, rule,
};
use crate::cli::handlers::outdated::refuses_offline_rule;
use crate::cli::handlers::self_update::refuses_offline_self_update_rule;
use crate::cli::handlers::{
    clean, doctor, export, init, install, outdated, prune, resolve, search, self_update, status,
    update, validate, why,
};
use crate::cli::help::{DEFAULT_HELP_WIDTH, format_help};
use crate::cli::parser::{HELP_AFTER_ERROR, Parsed, Stop};
use crate::errors::{AmbitError, ExitCode};
use crate::util::env::Env;
use crate::version::VERSION;

/// Where a command's output goes, one line at a time.
pub trait Io {
    fn stdout(&mut self, line: &str);
    fn stderr(&mut self, line: &str);

    /// The columns usage wraps to on stdout (`error` false) or stderr (`error` true): the
    /// terminal's width when that stream is one, as commander's `getOutHelpWidth` and
    /// `getErrHelpWidth` read it. `None` means [`DEFAULT_HELP_WIDTH`].
    fn help_width(&self, error: bool) -> Option<usize> {
        let _ = error;
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

    fn help_width(&self, error: bool) -> Option<usize> {
        let size = if error {
            let stream = std::io::stderr();

            stream
                .is_terminal()
                .then(|| terminal_size::terminal_size_of(stream))
        } else {
            let stream = std::io::stdout();

            stream
                .is_terminal()
                .then(|| terminal_size::terminal_size_of(stream))
        };

        size.flatten()
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

/// Handlers, keyed by the words a user types. Every command the surface declares has one here; a
/// command added without an entry reports itself unimplemented (exit 1) rather than silently
/// succeeding.
///
/// Entries have no spaces: the surface is flat. A group, were one declared, would still have no
/// entry here, since it holds commands and runs none itself.
pub fn handlers() -> CommandHandlers {
    CommandHandlers::from_iter([
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

/// Flag rules, keyed by the same words [`handlers`] is: what each command refuses about the flags
/// it was given, before dispatch.
///
/// Three commands need one, and all three refuse `--offline`. `outdated` and `update` share a rule,
/// since both refuse for the same reason: only a remote knows where a ref points now. `self-update`
/// refuses for a different reason (no cache holds a binary it has not downloaded), so it carries
/// its own wording. Rules exist instead of a parser-level "mandatory option" because that produces
/// a message that names no file and gives no next step. `install`'s `--copy`/`--link` still uses a
/// declared conflict, since the parser's wording for two flags that cannot appear together already
/// says everything needed.
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
/// Usage errors print `error: …` then ``(run `ambit --help` for usage)`` to stderr, exit 2.
/// `--help` and bare `ambit` print usage to stdout, exit 0. An [`AmbitError`] prints its
/// `format()` and exits with its code.
pub fn run_with(
    argv: &[String],
    cwd: &Path,
    env: &Env,
    io: &mut dyn Io,
    handlers: &CommandHandlers,
    rules: &CommandRules,
) -> ExitCode {
    run_surface(&command_specs(), argv, cwd, env, io, handlers, rules)
}

/// [`run_with`] against an arbitrary surface, so the group seam no shipped command uses can be
/// exercised.
pub fn run_surface(
    specs: &[CommandSpec],
    argv: &[String],
    cwd: &Path,
    env: &Env,
    io: &mut dyn Io,
    handlers: &CommandHandlers,
    rules: &CommandRules,
) -> ExitCode {
    let program = parser::program(specs, VERSION);
    let width = |io: &dyn Io, error: bool| io.help_width(error).unwrap_or(DEFAULT_HELP_WIDTH);

    // Bare `ambit` is a request for usage, not a mistake.
    if argv.is_empty() {
        io.stdout(&format_help(&program, width(io, false)));

        return ExitCode::Success;
    }

    let (command, options, args) = match parser::parse(&program, argv) {
        Ok(Parsed::Action {
            command,
            options,
            args,
        }) => (command, options, args),
        Ok(Parsed::Nothing) => return ExitCode::Success,
        Err(Stop::Help(command)) => {
            io.stdout(&format_help(command, width(io, false)));

            return ExitCode::Success;
        }
        Err(Stop::HelpError(command)) => {
            io.stderr(&format_help(command, width(io, true)));

            return ExitCode::Config;
        }
        Err(Stop::Version(version)) => {
            io.stdout(&version);

            return ExitCode::Success;
        }
        Err(Stop::Usage(message)) => {
            io.stderr(&message);
            io.stderr(HELP_AFTER_ERROR);

            return ExitCode::Config;
        }
    };

    // A group is a request for usage, not a mistake, exactly like bare `ambit`.
    if !command.acts {
        io.stdout(&format_help(command, width(io, false)));

        return ExitCode::Success;
    }

    let mut ctx = CommandContext {
        options,
        args,
        cwd: cwd.to_path_buf(),
        env,
        io,
    };

    match dispatch(&command.key, &mut ctx, handlers, rules) {
        Ok(code) => code,
        Err(error) => {
            ctx.io.stderr(&error.format());

            error.code
        }
    }
}

/// Runs the command's rule, then its handler. Only an acting command carries a rule, so a rule
/// runs exactly once, for the command it belongs to.
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
