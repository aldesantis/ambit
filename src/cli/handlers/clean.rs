use crate::cli::commands::{CommandContext, dry_run_requested, json_requested, project_dir_of};
use crate::cli::handlers::artifacts::{artifact_json, removal_rows};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result};
use crate::model::state::{STATE_DIRNAME, STATE_FILENAME};
use crate::project::clean::{CleanOptions, CleanResult, clean_project};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

fn block_path(file: &str) -> String {
    format!("{file} (managed block)")
}

fn to_json(result: &CleanResult) -> JsonObject {
    let mut record = JsonObject::new();

    record.insert(
        "gitignoreRemoved".to_owned(),
        JsonValue::Array(
            result
                .gitignore_removed
                .iter()
                .map(|file| JsonValue::String(file.clone()))
                .collect(),
        ),
    );
    record.insert(
        "removed".to_owned(),
        JsonValue::Array(
            result
                .removed
                .iter()
                .map(|artifact| JsonValue::Object(artifact_json(artifact)))
                .collect(),
        ),
    );
    record.insert(
        "stateRemoved".to_owned(),
        JsonValue::Bool(result.state_removed),
    );
    record
}

fn record_rows(result: &CleanResult) -> Vec<Vec<String>> {
    let mut rows = Vec::new();

    if result.state_removed {
        rows.push(vec![format!("{STATE_DIRNAME}/{STATE_FILENAME}")]);
    }

    rows.extend(
        result
            .gitignore_removed
            .iter()
            .map(|file| vec![block_path(file)]),
    );
    rows
}

fn to_text(result: &CleanResult) -> Vec<String> {
    let mut lines = section("removed", &removal_rows(&result.removed));
    lines.extend(section("records", &record_rows(result)));
    lines
}

pub fn clean_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let result = clean_project(
        &project_dir_of(ctx),
        ctx.env,
        CleanOptions {
            dry_run: dry_run_requested(ctx),
        },
    )?;

    if json_requested(ctx) {
        ctx.io
            .stdout(&stringify_pretty(&JsonValue::Object(to_json(&result))));
    } else {
        print_sections(&to_text(&result), ctx.io);
    }

    Ok(ExitCode::Success)
}
