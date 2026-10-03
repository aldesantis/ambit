//! The program: parsing argv against the declared surface, dispatching to a handler, and turning
//! every outcome into an exit code.
//!
//! The parser (`parser.rs`) and help renderer (`help.rs`) are a hand port of the subset of
//! commander 15 the TypeScript build used, so usage output and error wording stay byte-identical.

pub mod commands;
pub mod handlers;
mod help;
pub mod output;
mod parser;

use std::io::Write as _;
use std::path::Path;

use crate::cli::commands::{CommandHandlers, CommandRules};
use crate::errors::ExitCode;
use crate::util::env::Env;

/// Where a command's output goes, one line at a time.
pub trait Io {
    fn stdout(&mut self, line: &str);
    fn stderr(&mut self, line: &str);
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
}

/// Output captured line by line, for tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CaptureIo {
    pub out: Vec<String>,
    pub err: Vec<String>,
}

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
pub fn handlers() -> CommandHandlers {
    todo!("port cli/program.ts:HANDLERS")
}

/// Flag rules, keyed by the same words [`handlers`] is: what each command refuses about the flags
/// it was given, before dispatch.
///
/// Three commands need one, and all three refuse `--offline`. `outdated` and `update` share a rule;
/// `self-update` refuses for a different reason and carries its own wording.
pub fn rules() -> CommandRules {
    todo!("port cli/program.ts:RULES")
}

/// Runs the CLI with the shipped handlers and rules, and returns the process exit code.
pub fn run(argv: &[String], cwd: &Path, env: &Env, io: &mut dyn Io) -> ExitCode {
    run_with(argv, cwd, env, io, &handlers(), &rules())
}

/// Runs the CLI and returns the process exit code. Never fails: every failure path is translated
/// into an exit code, with the message already printed.
///
/// Usage errors print `error: …` then ``(run `ambit --help` for usage)`` to stderr, exit 2.
/// `--help` and bare `ambit` print usage to stdout, exit 0. An [`AmbitError`](crate::errors::AmbitError)
/// prints its `format()` and exits with its code.
pub fn run_with(
    argv: &[String],
    cwd: &Path,
    env: &Env,
    io: &mut dyn Io,
    handlers: &CommandHandlers,
    rules: &CommandRules,
) -> ExitCode {
    let _ = (argv, cwd, env, io, handlers, rules);
    todo!("port cli/program.ts:run")
}
