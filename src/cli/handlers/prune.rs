//! `ambit prune`: remove owned artifacts not in the current bundle.
//!
//! The report is what was removed, not what survived: `ambit status` is where to see everything
//! still installed. A run with nothing to remove says so explicitly (`(none)` under a counted
//! heading) rather than printing nothing, so a quiet prune is distinguishable from a prune that did
//! not run.

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

/// # Errors
///
/// Whatever the prune returns, already in the standard message shape.
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
