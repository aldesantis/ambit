//! The `ambit` binary: a deterministic dependency manager for AI-agent capabilities.
//!
//! `main` is the only place that touches process state: it snapshots the environment and the cwd,
//! hands them to [`cli::run`], and turns the returned code into the exit status. Everything below
//! takes them as arguments.
// Temporary while modules are skeletons; removed in Wave 3.
#![allow(dead_code, unused_imports)]
#![allow(clippy::todo, clippy::unimplemented)]

mod cli;
mod errors;
mod export;
mod harness;
mod model;
mod project;
mod resolution;
mod self_update;
mod util;
mod version;

#[cfg(test)]
mod test_support;

use crate::errors::AmbitError;

#[allow(clippy::disallowed_methods)]
fn main() {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let env = util::env::snapshot();
    let mut io = cli::StdIo;

    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            cli::Io::stderr(&mut io, &AmbitError::unexpected(error).format());
            std::process::exit(errors::ExitCode::Internal.as_i32());
        }
    };

    let code = cli::run(&argv, &cwd, &env, &mut io);

    std::process::exit(code.as_i32());
}
