use crate::cli::commands::{CommandContext, json_requested, offline_requested, project_dir_of};
use crate::cli::handlers::pins::{diff_json, diff_sections, pin_json, pin_rows};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result, network_error};
use crate::project::bundle_diff::{BundleDiff, is_unchanged};
use crate::project::update::{
    CatalogPin, UpdateOptions, UpdatePlan, catalogs_outdated, check_outdated,
};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

pub fn refuses_offline_rule(ctx: &CommandContext<'_>) -> Result<()> {
    if !offline_requested(ctx) {
        return Ok(());
    }

    Err(network_error(
        "`--offline` cannot answer where a ref points now",
        [
            "this command asks each catalog's remote for its current commit, which the cache cannot know",
            "run the command again without `--offline`",
        ],
    ))
}

pub(crate) fn plan_json(catalogs: &[CatalogPin], diff: &BundleDiff) -> JsonObject {
    let mut record = JsonObject::new();

    record.insert("catalogs".to_owned(), JsonValue::Object(pin_json(catalogs)));
    record.insert("changed".to_owned(), JsonValue::Bool(!is_unchanged(diff)));
    record.extend(diff_json(diff));
    record.insert(
        "outdated".to_owned(),
        JsonValue::Bool(catalogs_outdated(catalogs)),
    );
    record
}

pub fn plan_text(plan: &UpdatePlan) -> Vec<String> {
    pins_text(&plan.catalogs, &plan.diff)
}

pub(crate) fn pins_text(catalogs: &[CatalogPin], diff: &BundleDiff) -> Vec<String> {
    let mut lines = section("catalogs", &pin_rows(catalogs));
    lines.extend(diff_sections(diff));
    lines
}

pub fn outdated_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let plan = check_outdated(&project_dir_of(ctx), ctx.env, &UpdateOptions::default())?;

    if json_requested(ctx) {
        let json = plan_json(&plan.catalogs, &plan.diff);
        ctx.io.stdout(&stringify_pretty(&JsonValue::Object(json)));
    } else {
        print_sections(&plan_text(&plan), ctx.io);
    }

    Ok(ExitCode::Success)
}
