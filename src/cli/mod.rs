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

// Fixed so clap never reads the terminal or `COLUMNS` itself.
pub const MAX_HELP_WIDTH: usize = 100;

pub trait Io {
    fn stdout(&mut self, line: &str);
    fn stderr(&mut self, line: &str);

    fn help_width(&self) -> Option<usize> {
        None
    }
}

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

pub fn run(argv: &[String], cwd: &Path, env: &Env, io: &mut dyn Io) -> ExitCode {
    run_with(argv, cwd, env, io, &handlers(), &rules())
}

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
            _ => None,
        };

        if let Some(value) = value {
            options.insert(id.to_owned(), value);
        }
    }

    (name.to_owned(), CommandOptions(options), args)
}

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
