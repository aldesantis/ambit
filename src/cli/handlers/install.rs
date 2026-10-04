//! `ambit install`: resolve, write the lock, materialize, record ownership.
//!
//! Output names artifacts by their project-relative path, so it is comparable between machines.
//! The lock is not among them: it's a record of the resolution, not an owned artifact, so nothing
//! prunes it and it is not ambit's to delete.
//!
//! `--dry-run` prints the same two sections the install would print, plus what only a preview can
//! usefully say: what install would remove, and whether `ambit.lock` and each managed `.gitignore`
//! block would change. The artifact rows match the real run's shape so the two outputs diff
//! cleanly.
//!
//! A hook a configured harness cannot express is a warning on stderr, and exit stays 0. Stderr
//! because stdout is the report a script parses and a skip isn't part of what was installed. A
//! warning, not an error, because the hook did install everywhere else; failing would let one
//! harness veto every other harness's hooks.

use serde_json::json;

use crate::cli::commands::{
    CommandContext, dry_run_requested, json_requested, offline_requested, project_dir_of,
};
use crate::cli::handlers::artifacts::{
    ReportedArtifact, artifact_json, artifact_rows, removal_rows,
};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result};
use crate::harness::adapter::{HookSkipReason, SkippedHook};
use crate::model::state::ArtifactMode;
use crate::project::install::{
    InstallOptions, InstallPreview, InstallResult, install_project, preview_install,
};
use crate::project::lock::LOCK_FILENAME;
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

/// `--copy` / `--link`, as the materialization mode they force.
///
/// `None` (neither flag) means the mode follows each skill's source; it is the absence of an
/// override, not a third mode value.
///
/// The two together never reach here: they're declared as conflicting options
/// (`cli/commands.rs`), so the parser refuses the invocation with exit 2 before any handler runs.
fn mode_override(ctx: &CommandContext<'_>) -> Option<ArtifactMode> {
    if ctx.options.flag("copy") {
        return Some(ArtifactMode::Copy);
    }

    if ctx.options.flag("link") {
        return Some(ArtifactMode::Link);
    }

    None
}

/// Every flag `install_project` and `preview_install` share, so the two paths cannot diverge.
fn options_of(ctx: &CommandContext<'_>) -> InstallOptions {
    InstallOptions {
        frozen: ctx.options.flag("frozen"),
        offline: offline_requested(ctx),
        adopt: ctx.options.flag("adopt"),
        mode: mode_override(ctx),
    }
}

/// Why one harness could not take one hook, in a sentence.
///
/// The two reasons read differently on purpose: one is about the harness as a whole ("opencode
/// will never run this"), the other about one event it has no counterpart for ("Kiro has no
/// trigger for compaction").
fn skip_reason(skipped: &SkippedHook) -> String {
    match skipped.reason {
        HookSkipReason::NoMechanism => {
            format!("{} has no declarative hook mechanism", skipped.harness)
        }
        HookSkipReason::NoEvent => format!(
            "{} has no spelling for the {} event",
            skipped.harness, skipped.event
        ),
    }
}

/// One line per skipped hook, named the way its declaration names it.
///
/// Shared with `ambit update`, which ends in an install and owes the same warning: a hook a harness
/// cannot express is still skipped even when it arrived through an updated catalog.
pub fn skip_warnings(skipped: &[SkippedHook]) -> Vec<String> {
    skipped
        .iter()
        .map(|skip| {
            format!(
                "warning: hook \"{}\" ({}) not installed: {}",
                skip.hook,
                skip.event,
                skip_reason(skip)
            )
        })
        .collect()
}

/// One skipped hook as a JSON record. Carries the reason kind, not the sentence; wording is the
/// text renderer's job.
pub fn skip_json(skipped: &SkippedHook) -> JsonObject {
    let mut record = JsonObject::new();

    record.insert("event".to_owned(), json!(skipped.event.as_str()));
    record.insert("harness".to_owned(), json!(skipped.harness));
    record.insert("hook".to_owned(), json!(skipped.hook));
    record.insert("reason".to_owned(), json!(skipped.reason.as_str()));
    record
}

/// Each artifact as a JSON record, as a JSON array.
fn artifacts_json<A: ReportedArtifact>(artifacts: &[A]) -> JsonValue {
    JsonValue::Array(
        artifacts
            .iter()
            .map(|artifact| JsonValue::Object(artifact_json(artifact)))
            .collect(),
    )
}

fn skipped_json(skipped: &[SkippedHook]) -> JsonValue {
    JsonValue::Array(
        skipped
            .iter()
            .map(|skip| JsonValue::Object(skip_json(skip)))
            .collect(),
    )
}

fn harness_rows(harnesses: &[String]) -> Vec<Vec<String>> {
    harnesses
        .iter()
        .map(|harness| vec![harness.clone()])
        .collect()
}

fn to_json(result: &InstallResult) -> JsonValue {
    json!({
        "artifacts": artifacts_json(&result.artifacts),
        "harnesses": result.harnesses,
        "skills": result.bundle.skills.iter().map(|skill| skill.name.as_str()).collect::<Vec<_>>(),
        "skipped": skipped_json(&result.skipped),
    })
}

fn to_text(result: &InstallResult) -> Vec<String> {
    let mut lines = section("harnesses", &harness_rows(&result.harnesses));

    lines.extend(section("artifacts", &artifact_rows(&result.artifacts)));
    lines
}

fn changed(changed: bool) -> String {
    if changed { "changed" } else { "unchanged" }.to_owned()
}

fn preview_json(preview: &InstallPreview) -> JsonValue {
    json!({
        "artifacts": artifacts_json(&preview.artifacts),
        "gitignore": preview
            .gitignore
            .iter()
            .map(|block| json!({ "changed": block.changed, "file": block.file }))
            .collect::<Vec<_>>(),
        "harnesses": preview.harnesses,
        "lockChanged": preview.lock_changed,
        "pruned": artifacts_json(&preview.pruned),
        "skills": preview.bundle.skills.iter().map(|skill| skill.name.as_str()).collect::<Vec<_>>(),
        "skipped": skipped_json(&preview.skipped),
    })
}

fn preview_text(preview: &InstallPreview) -> Vec<String> {
    let mut files = vec![vec![
        LOCK_FILENAME.to_owned(),
        changed(preview.lock_changed),
    ]];

    files.extend(
        preview
            .gitignore
            .iter()
            .map(|block| vec![block.file.clone(), changed(block.changed)]),
    );

    let mut lines = section("harnesses", &harness_rows(&preview.harnesses));

    lines.extend(section("artifacts", &artifact_rows(&preview.artifacts)));
    lines.extend(section("pruned", &removal_rows(&preview.pruned)));
    lines.extend(section("files", &files));
    lines
}

/// # Errors
///
/// Whatever the install returns, already in the standard message shape.
pub fn install_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let options = options_of(ctx);
    let project_dir = project_dir_of(ctx);

    if dry_run_requested(ctx) {
        let preview = preview_install(&project_dir, ctx.env, options)?;

        if json_requested(ctx) {
            ctx.io.stdout(&stringify_pretty(&preview_json(&preview)));
        } else {
            print_sections(&preview_text(&preview), ctx.io);
        }

        for line in skip_warnings(&preview.skipped) {
            ctx.io.stderr(&line);
        }

        return Ok(ExitCode::Success);
    }

    let result = install_project(&project_dir, ctx.env, options, &[])?;

    if json_requested(ctx) {
        ctx.io.stdout(&stringify_pretty(&to_json(&result)));
    } else {
        print_sections(&to_text(&result), ctx.io);
    }

    for line in skip_warnings(&result.skipped) {
        ctx.io.stderr(&line);
    }

    Ok(ExitCode::Success)
}
