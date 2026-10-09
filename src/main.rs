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
#[cfg(test)]
mod tests;

use std::io::IsTerminal as _;
use std::panic::{self, AssertUnwindSafe};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::Io as _;
use crate::errors::{AmbitError, ExitCode};
use crate::self_update::notice::{NoticeContext, update_notice};
use crate::self_update::release::UreqHttp;
use crate::util::env::Env;

#[allow(clippy::disallowed_methods)]
fn main() {
    let argv: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let env = util::env::snapshot();
    let mut io = cli::StdIo;

    panic::set_hook(Box::new(|_| {}));

    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            io.stderr(&AmbitError::unexpected(error).format());
            std::process::exit(ExitCode::Internal.as_i32());
        }
    };

    let code = match panic::catch_unwind(AssertUnwindSafe(|| cli::run(&argv, &cwd, &env, &mut io)))
    {
        Ok(code) => code,
        Err(payload) => {
            io.stderr(&AmbitError::unexpected(panic_message(payload.as_ref())).format());
            ExitCode::Internal
        }
    };

    if code == ExitCode::Success
        && let Some(notice) = notice(&env, &argv)
    {
        io.stderr(&notice);
    }

    std::process::exit(code.as_i32());
}

// Kept out of `cli::run` so the test suite never reaches the network.
fn notice(env: &Env, argv: &[String]) -> Option<String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    let http = UreqHttp;
    let context = NoticeContext {
        env,
        argv,
        is_tty: std::io::stderr().is_terminal(),
        now,
        http: &http,
    };

    panic::catch_unwind(AssertUnwindSafe(|| update_notice(&context)))
        .ok()
        .flatten()
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic with a non-string payload".to_owned()
    }
}
