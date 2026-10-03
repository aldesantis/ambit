//! `ambit init`: scaffold a project, which is also a catalog.
//!
//! Two counted sections, not one line: "created" and "kept" are different news, since a project is
//! routinely initialized inside a repo that already has a `skills/` directory and a reader needs to
//! see that theirs was left alone. Both sections print even when empty, so a quiet run is
//! distinguishable from a run that did nothing.
//!
//! `--dry-run` prints the config's bytes instead of the next step, since here the plan is the
//! bytes. The `.gitkeep` files have nothing further to show; their whole content is the path
//! already listed above. `--json` carries every file either way, so a consuming tool can write the
//! scaffold itself.

use serde_json::json;

use crate::cli::commands::{CommandContext, dry_run_requested, json_requested, project_dir_of};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result};
use crate::project::init::{InitOptions, InitResult, init_project};
use crate::util::json::{JsonValue, stringify_pretty};
use crate::util::text::js_trim_end;

/// The two things left to do, in the order they have to happen in.
///
/// A scaffolded project selects nothing: its `requires` block is commented out, because an entry
/// matching nothing is exit 3 and its own `local` catalog starts empty. `ambit install` on it would
/// install nothing, so these steps say so up front rather than let it be a surprise.
const NEXT_STEPS: &[&str] = &[
    "next: put a skill in `skills/<name>/SKILL.md`, or add a catalog under `catalogs`",
    "      then uncomment a `requires` entry that selects it, and run `ambit install`",
];

fn to_json(result: &InitResult) -> JsonValue {
    let created: Vec<JsonValue> = result
        .created
        .iter()
        .map(|scaffolded| json!({ "file": scaffolded.file, "text": scaffolded.text }))
        .collect();

    json!({
        "created": created,
        "kept": result.kept,
        "written": result.written,
    })
}

fn rows<'a>(files: impl Iterator<Item = &'a String>) -> Vec<Vec<String>> {
    files.map(|file| vec![file.clone()]).collect()
}

/// The bytes `--dry-run` withheld: every created file that has any, which is the config.
fn preview(result: &InitResult) -> Vec<String> {
    result
        .created
        .iter()
        .filter(|scaffolded| !scaffolded.text.is_empty())
        .flat_map(|scaffolded| [js_trim_end(&scaffolded.text).to_owned(), String::new()])
        .collect()
}

fn to_text(result: &InitResult, dry_run: bool) -> Vec<String> {
    let mut lines = section(
        if dry_run { "would create" } else { "created" },
        &rows(result.created.iter().map(|scaffolded| &scaffolded.file)),
    );

    lines.extend(section("kept", &rows(result.kept.iter())));

    if dry_run {
        lines.extend(preview(result));
    } else {
        lines.extend(NEXT_STEPS.iter().map(|&line| line.to_owned()));
        lines.push(String::new());
    }

    lines
}

/// # Errors
///
/// Whatever the init returns, already in the standard message shape.
pub fn init_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let dry_run = dry_run_requested(ctx);
    let result = init_project(&project_dir_of(ctx), InitOptions { dry_run })?;

    if json_requested(ctx) {
        ctx.io.stdout(&stringify_pretty(&to_json(&result)));
    } else {
        print_sections(&to_text(&result, dry_run), ctx.io);
    }

    Ok(ExitCode::Success)
}
