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
use crate::project::audit::{AuditFinding, item_label};
use crate::project::exec::ExecChange;
use crate::project::install::{
    InstallOptions, InstallPreview, InstallResult, install_project, preview_install,
};
use crate::project::lock::LOCK_FILENAME;
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

fn mode_override(ctx: &CommandContext<'_>) -> Option<ArtifactMode> {
    if ctx.options.flag("copy") {
        return Some(ArtifactMode::Copy);
    }

    if ctx.options.flag("link") {
        return Some(ArtifactMode::Link);
    }

    None
}

fn options_of(ctx: &CommandContext<'_>) -> InstallOptions {
    InstallOptions {
        frozen: ctx.options.flag("frozen"),
        offline: offline_requested(ctx),
        adopt: ctx.options.flag("adopt"),
        mode: mode_override(ctx),
        no_audit: ctx.options.flag("noAudit"),
        accept_exec: ctx.options.flag("acceptExec"),
    }
}

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

pub fn audit_warnings(findings: &[AuditFinding]) -> Vec<String> {
    findings
        .iter()
        .map(|finding| format!("warning: {}: {}", item_label(finding), finding.message))
        .collect()
}

pub fn endpoint_warnings(endpoints: &[ExecChange]) -> Vec<String> {
    endpoints
        .iter()
        .map(|change| {
            format!(
                "warning: {} \"{}\" connects to {} ({})",
                change.kind,
                change.name,
                change.runs,
                change.note()
            )
        })
        .collect()
}

pub fn skip_json(skipped: &SkippedHook) -> JsonObject {
    let mut record = JsonObject::new();

    record.insert("event".to_owned(), json!(skipped.event.as_str()));
    record.insert("harness".to_owned(), json!(skipped.harness));
    record.insert("hook".to_owned(), json!(skipped.hook));
    record.insert("reason".to_owned(), json!(skipped.reason.as_str()));
    record
}

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

        for line in skip_warnings(&preview.skipped)
            .into_iter()
            .chain(endpoint_warnings(&preview.endpoints))
            .chain(audit_warnings(&preview.audit))
        {
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

    for line in skip_warnings(&result.skipped)
        .into_iter()
        .chain(endpoint_warnings(&result.endpoints))
        .chain(audit_warnings(&result.audit))
    {
        ctx.io.stderr(&line);
    }

    Ok(ExitCode::Success)
}
