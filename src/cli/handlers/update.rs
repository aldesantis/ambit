use crate::cli::commands::{CommandContext, dry_run_requested, json_requested, project_dir_of};
use crate::cli::handlers::artifacts::{artifact_json, artifact_rows};
use crate::cli::handlers::install::{audit_warnings, endpoint_warnings, skip_json, skip_warnings};
use crate::cli::handlers::outdated::{pins_text, plan_json};
use crate::cli::handlers::pins::{diff_json, pin_json};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result};
use crate::model::state::ArtifactMode;
use crate::project::bundle_diff::is_unchanged;
use crate::project::update::{
    UpdateInstallOptions, UpdateOptions, UpdateResult, catalogs_outdated, preview_update,
    update_project,
};
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

fn install_options_of(ctx: &CommandContext<'_>) -> UpdateInstallOptions {
    UpdateInstallOptions {
        adopt: ctx.options.flag("adopt"),
        mode: mode_override(ctx),
        no_audit: ctx.options.flag("noAudit"),
        accept_exec: ctx.options.flag("acceptExec"),
    }
}

fn to_json(result: &UpdateResult) -> JsonObject {
    let mut record = JsonObject::new();

    record.insert(
        "artifacts".to_owned(),
        JsonValue::Array(
            result
                .install
                .artifacts
                .iter()
                .map(|artifact| JsonValue::Object(artifact_json(artifact)))
                .collect(),
        ),
    );
    record.insert(
        "catalogs".to_owned(),
        JsonValue::Object(pin_json(&result.catalogs)),
    );
    record.insert(
        "changed".to_owned(),
        JsonValue::Bool(!is_unchanged(&result.diff)),
    );
    record.insert(
        "harnesses".to_owned(),
        JsonValue::Array(
            result
                .install
                .harnesses
                .iter()
                .map(|harness| JsonValue::String(harness.clone()))
                .collect(),
        ),
    );
    record.extend(diff_json(&result.diff));
    record.insert(
        "outdated".to_owned(),
        JsonValue::Bool(catalogs_outdated(&result.catalogs)),
    );
    record.insert(
        "skipped".to_owned(),
        JsonValue::Array(
            result
                .install
                .skipped
                .iter()
                .map(|skipped| JsonValue::Object(skip_json(skipped)))
                .collect(),
        ),
    );
    record
}

fn to_text(result: &UpdateResult) -> Vec<String> {
    let harnesses: Vec<Vec<String>> = result
        .install
        .harnesses
        .iter()
        .map(|harness| vec![harness.clone()])
        .collect();
    let mut lines = pins_text(&result.catalogs, &result.diff);

    lines.extend(section("harnesses", &harnesses));
    lines.extend(section(
        "artifacts",
        &artifact_rows(&result.install.artifacts),
    ));
    lines
}

pub fn update_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let project_dir = project_dir_of(ctx);
    let options = UpdateOptions {
        catalogs: ctx.args.clone(),
    };

    if dry_run_requested(ctx) {
        let plan = preview_update(&project_dir, ctx.env, &options)?;

        if json_requested(ctx) {
            let json = plan_json(&plan.catalogs, &plan.diff);
            ctx.io.stdout(&stringify_pretty(&JsonValue::Object(json)));
        } else {
            print_sections(&pins_text(&plan.catalogs, &plan.diff), ctx.io);
        }

        return Ok(ExitCode::Success);
    }

    let result = update_project(&project_dir, ctx.env, &options, install_options_of(ctx))?;

    if json_requested(ctx) {
        ctx.io
            .stdout(&stringify_pretty(&JsonValue::Object(to_json(&result))));
    } else {
        print_sections(&to_text(&result), ctx.io);
    }

    for line in skip_warnings(&result.install.skipped)
        .into_iter()
        .chain(endpoint_warnings(&result.install.endpoints))
        .chain(audit_warnings(&result.install.audit))
    {
        ctx.io.stderr(&line);
    }

    Ok(ExitCode::Success)
}
