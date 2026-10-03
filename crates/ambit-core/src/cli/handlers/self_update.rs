//! `ambit self-update [version]`: replace this binary with a released one.
//!
//! The one command whose subject is ambit rather than a project, which is why it takes no
//! `--project` and why the version is a positional rather than a `--version` flag: `--version` is
//! already how the program prints its own.
//!
//! A named version is installed whether it is newer or older, so a bad release can be backed out
//! without hunting down the install script. Only the report says which direction it went.
//!
//! `--dry-run` prints the plan, which is the whole decision: the plan is made before anything is
//! downloaded, so it is also what every refusal comes out of.

use std::path::PathBuf;

use crate::cli::commands::{CommandContext, dry_run_requested, json_requested, offline_requested};
use crate::cli::output::columns;
use crate::errors::{ExitCode, Result, network_error};
use crate::self_update::release::{Http, UreqHttp};
use crate::self_update::update::{
    SelfContext, SelfUpdatePlan, apply_self_update, is_upgrade, plan_self_update,
};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

/// The refusal of `--offline`.
///
/// A rule rather than a check in the handler, so the parser enforces it before dispatch and a run
/// that cannot mean anything never starts. Worded for this command rather than shared with
/// `outdated` and `update`: those refuse because only a remote knows where a ref points now, and
/// this one refuses because the bytes it installs do not exist locally.
///
/// # Errors
///
/// Exit 4 when `--offline` was given.
pub fn refuses_offline_self_update_rule(ctx: &CommandContext<'_>) -> Result<()> {
    if !offline_requested(ctx) {
        return Ok(());
    }

    Err(network_error(
        "`--offline` cannot install a release",
        [
            "this command downloads a binary from GitHub, which no local cache holds",
            "run the command again without `--offline`",
        ],
    ))
}

/// What the machine looks like to self-update.
///
/// Built here, at the CLI boundary, for the same reason
/// [`source_context_of`](crate::cli::commands::source_context_of) is: one command run sees one
/// machine, and nothing further down reaches for ambient state of its own.
pub fn self_context_of(http: &dyn Http) -> SelfContext<'_> {
    SelfContext {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        // An executable whose own path cannot be read has nothing self-update could replace; the
        // empty path fails the writability check with a message naming it.
        exec_path: std::env::current_exe().unwrap_or_else(|_| PathBuf::new()),
        http,
    }
}

fn to_json(plan: &SelfUpdatePlan, installed: bool) -> JsonValue {
    let mut record = JsonObject::new();

    record.insert("asset".to_owned(), plan.asset.clone().into());
    record.insert(
        "binary".to_owned(),
        plan.binary.to_string_lossy().into_owned().into(),
    );
    record.insert("changed".to_owned(), plan.changed.into());
    record.insert("current".to_owned(), plan.current.clone().into());
    record.insert("installed".to_owned(), installed.into());
    record.insert("target".to_owned(), plan.target.clone().into());
    record.insert("upgrade".to_owned(), is_upgrade(plan).into());

    JsonValue::Object(record)
}

/// The last line: what happened, or what would have.
fn verdict(plan: &SelfUpdatePlan, dry_run: bool) -> String {
    if !plan.changed {
        return format!("ambit {} is already installed", plan.target);
    }

    if dry_run {
        return format!("would install ambit {}", plan.target);
    }

    if is_upgrade(plan) {
        return format!("installed ambit {}", plan.target);
    }

    format!(
        "installed ambit {}, a downgrade from {}",
        plan.target, plan.current
    )
}

fn to_text(plan: &SelfUpdatePlan, dry_run: bool) -> Vec<String> {
    let rows = [
        ["current".to_owned(), plan.current.clone()],
        ["target".to_owned(), plan.target.clone()],
        ["asset".to_owned(), plan.asset.clone()],
        [
            "binary".to_owned(),
            plan.binary.to_string_lossy().into_owned(),
        ],
    ];
    let mut lines = columns(&rows);

    lines.push(String::new());
    lines.push(verdict(plan, dry_run));
    lines
}

/// [`self_update_handler`] against a given machine, so a test can describe one.
///
/// # Errors
///
/// Whatever planning or applying the update returns.
pub(crate) fn run_self_update(
    ctx: &mut CommandContext<'_>,
    context: &SelfContext<'_>,
) -> Result<ExitCode> {
    let dry_run = dry_run_requested(ctx);
    let plan = plan_self_update(context, ctx.args.first().map(String::as_str))?;
    let installed = plan.changed && !dry_run;

    if installed {
        apply_self_update(&plan, context)?;
    }

    if json_requested(ctx) {
        ctx.io.stdout(&stringify_pretty(&to_json(&plan, installed)));
    } else {
        for line in to_text(&plan, dry_run) {
            ctx.io.stdout(&line);
        }
    }

    Ok(ExitCode::Success)
}

/// # Errors
///
/// Whatever planning or applying the update returns, already in the standard message shape.
pub fn self_update_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let http = UreqHttp;
    let context = self_context_of(&http);

    run_self_update(ctx, &context)
}

#[cfg(test)]
mod tests;
