use crate::cli::commands::{
    CommandContext, dry_run_requested, json_requested, offline_requested, project_dir_of,
};
use crate::cli::handlers::artifacts::{ReportedArtifact, artifact_json, removal_rows};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result};
use crate::project::clean::{PruneOptions, PruneResult, prune_project};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

fn json_list<A: ReportedArtifact>(artifacts: &[A]) -> JsonValue {
    JsonValue::Array(
        artifacts
            .iter()
            .map(|artifact| JsonValue::Object(artifact_json(artifact)))
            .collect(),
    )
}

fn to_json(result: &PruneResult) -> JsonObject {
    let mut record = JsonObject::new();

    record.insert("pruned".to_owned(), json_list(&result.pruned));
    record.insert("remaining".to_owned(), json_list(&result.remaining));
    record
}

fn to_text(result: &PruneResult) -> Vec<String> {
    section("pruned", &removal_rows(&result.pruned))
}

pub fn prune_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let result = prune_project(
        &project_dir_of(ctx),
        ctx.env,
        PruneOptions {
            offline: offline_requested(ctx),
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
