use serde_json::json;

use crate::cli::commands::{CommandContext, json_requested, offline_requested, project_dir_of};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result};
use crate::project::status::{
    ProjectStatus, StatusArtifact, StatusOptions, is_clean, project_status,
};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

fn artifact_json(artifact: &StatusArtifact) -> JsonValue {
    let mut record = JsonObject::new();

    if !artifact.detail.is_empty() {
        record.insert("detail".to_owned(), json!(artifact.detail));
    }

    record.insert("kind".to_owned(), json!(artifact.kind.as_str()));
    record.insert("path".to_owned(), json!(artifact.path));
    record.insert("state".to_owned(), json!(artifact.state.as_str()));
    JsonValue::Object(record)
}

fn to_json(status: &ProjectStatus) -> JsonValue {
    let artifacts: Vec<JsonValue> = status.artifacts.iter().map(artifact_json).collect();

    json!({
        "artifacts": artifacts,
        "clean": is_clean(status),
    })
}

fn to_text(status: &ProjectStatus) -> Vec<String> {
    let rows: Vec<Vec<String>> = status
        .artifacts
        .iter()
        .map(|artifact| {
            vec![
                artifact.path.clone(),
                artifact.kind.as_str().to_owned(),
                artifact.state.as_str().to_owned(),
                artifact.detail.clone(),
            ]
        })
        .collect();

    section("artifacts", &rows)
}

pub fn status_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let status = project_status(
        &project_dir_of(ctx),
        ctx.env,
        StatusOptions {
            offline: offline_requested(ctx),
        },
    )?;

    if json_requested(ctx) {
        ctx.io.stdout(&stringify_pretty(&to_json(&status)));
    } else {
        print_sections(&to_text(&status), ctx.io);
    }

    if !ctx.options.flag("check") || is_clean(&status) {
        return Ok(ExitCode::Success);
    }

    Ok(ExitCode::Drift)
}
