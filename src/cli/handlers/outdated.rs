//! `ambit outdated`: where every catalog's pin stands, and what moving it would bring.
//!
//! Four sections rather than a list of stale catalogs: the first says which pins have somewhere to
//! go, and the three after it say what going there would actually change. A catalog can be many
//! commits ahead while the bundle doesn't move at all, and only the second half of the report can
//! show that.
//!
//! Reaches the remote but leaves the cache's own refs alone, so running it never changes what a
//! later `ambit install` installs. Under `--offline` it refuses rather than reporting `current`
//! from a cache that has no way to know that.
//!
//! Exit 0 whatever it finds: being behind is a fact, not a failure. `--json` carries `outdated` and
//! `changed` for a script that wants to branch on it.

use crate::cli::commands::{CommandContext, json_requested, offline_requested, project_dir_of};
use crate::cli::handlers::pins::{diff_json, diff_sections, pin_json, pin_rows};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result, network_error};
use crate::project::bundle_diff::{BundleDiff, is_unchanged};
use crate::project::update::{
    CatalogPin, UpdateOptions, UpdatePlan, catalogs_outdated, check_outdated,
};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

/// Both `outdated`'s and `update`'s refusal of `--offline`.
///
/// A rule, not a check inside the handler, so it is enforced before dispatch and a run that cannot
/// mean anything never starts. Refuses rather than silently falling back to the cache: only the
/// remote knows where a branch points now, and a cached commit reported as current is worse than no
/// report at all.
///
/// # Errors
///
/// Exit 4 when `--offline` was given.
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

/// The JSON report both `outdated` and `update --dry-run` print, led by whether every namespace
/// agrees the bundle would not move.
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

/// The four sections, catalogs first.
///
/// Every configured catalog is listed, not only the moved ones: "your other three are current" is
/// part of the answer, and a report of only problems couldn't distinguish a clean project from one
/// it forgot to check.
pub fn plan_text(plan: &UpdatePlan) -> Vec<String> {
    pins_text(&plan.catalogs, &plan.diff)
}

/// [`plan_text`] over the two parts, for `update`'s result, which carries them unbundled.
pub(crate) fn pins_text(catalogs: &[CatalogPin], diff: &BundleDiff) -> Vec<String> {
    let mut lines = section("catalogs", &pin_rows(catalogs));
    lines.extend(diff_sections(diff));
    lines
}

/// # Errors
///
/// Whatever checking the pins returns, already in the standard message shape.
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
