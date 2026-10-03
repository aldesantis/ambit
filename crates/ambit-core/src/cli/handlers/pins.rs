//! How `outdated` and `update` render where a pin stands and what moving it changes.
//!
//! `outdated` and `update` print the same report (`update` adds what the install then did), so the
//! projections live here once rather than in each command.
//!
//! The commit column is abbreviated: a full pair of forty-hex strings pushes everything worth
//! reading off the line, and `ambit.lock` already holds the exact commits for anyone who needs them.

use serde_json::json;

use crate::cli::output::{keyed, section};
use crate::project::bundle_diff::{BundleChange, BundleChangeKind, BundleDiff, count_changes};
use crate::project::update::CatalogPin;
use crate::util::json::{JsonObject, JsonValue};

/// Stands in for a column a `path:` catalog has nothing to put in: it has no revision.
const NO_COMMIT: &str = "-";

/// How many hex characters of a commit a report shows: git's own conventional abbreviation.
const ABBREVIATED: usize = 7;

/// The marker column, so a reader scans one character rather than reading a word three times.
fn marker(change: BundleChangeKind) -> &'static str {
    match change {
        BundleChangeKind::Added => "+",
        BundleChangeKind::Changed => "~",
        BundleChangeKind::Removed => "-",
    }
}

fn abbreviate(commit: &str) -> String {
    commit.chars().take(ABBREVIATED).collect()
}

/// Where a pin stands, in one cell: the commit, or the move it would make.
///
/// A catalog that has not moved shows one commit rather than the same one twice: the arrow means
/// change, so printing it where nothing changed would make every row look like one.
fn transition_of(pin: &CatalogPin) -> String {
    let Some(commit) = &pin.commit else {
        return NO_COMMIT.to_owned();
    };

    match &pin.latest {
        Some(latest) if latest != commit => {
            format!("{} → {}", abbreviate(commit), abbreviate(latest))
        }
        _ => abbreviate(commit),
    }
}

/// One row per configured catalog: its name, where it stands, and the commit or the move.
pub fn pin_rows(pins: &[CatalogPin]) -> Vec<Vec<String>> {
    pins.iter()
        .map(|pin| {
            vec![
                pin.name.clone(),
                pin.freshness.as_str().to_owned(),
                transition_of(pin),
            ]
        })
        .collect()
}

/// Every pin as a name-keyed JSON record.
///
/// Full commits here, unlike the text form: a consumer comparing this against `ambit.lock` needs
/// the exact value the lock holds.
pub fn pin_json(pins: &[CatalogPin]) -> JsonObject {
    keyed(
        pins,
        |pin| pin.name.clone(),
        |pin| {
            let mut record = JsonObject::new();

            if let Some(commit) = &pin.commit {
                record.insert("commit".to_owned(), json!(commit));
            }

            record.insert("freshness".to_owned(), json!(pin.freshness.as_str()));

            if let Some(latest) = &pin.latest {
                record.insert("latest".to_owned(), json!(latest));
            }

            if let Some(r#ref) = &pin.r#ref {
                record.insert("ref".to_owned(), json!(r#ref));
            }

            record.insert("source".to_owned(), json!(pin.source));
            JsonValue::Object(record)
        },
    )
}

/// One row per change: what happened, to what, and the one line a reader can act on.
fn change_rows(changes: &[BundleChange]) -> Vec<Vec<String>> {
    changes
        .iter()
        .map(|change| {
            vec![
                marker(change.change).to_owned(),
                change.name.clone(),
                change.detail.clone(),
            ]
        })
        .collect()
}

/// One namespace's changes as JSON records, in the order the text form lists them.
///
/// A list rather than a name-keyed map, unlike everything else ambit emits: the counts are what a
/// consumer reads first, and a map would make them count keys by hand.
fn change_json(changes: &[BundleChange]) -> JsonValue {
    let counts = count_changes(changes);
    let list: Vec<JsonValue> = changes
        .iter()
        .map(|change| {
            json!({
                "change": change.change.as_str(),
                "detail": change.detail,
                "name": change.name,
            })
        })
        .collect();

    json!({
        "changes": list,
        "added": counts.added,
        "changed": counts.changed,
        "removed": counts.removed,
    })
}

/// The whole bundle diff as one section per namespace, in the order every other report lists them.
///
/// Packs lead even though a pack materializes nothing: a pack whose membership moved is the cause
/// of most of the rows below it.
pub fn diff_sections(diff: &BundleDiff) -> Vec<String> {
    let mut lines = section("packs", &change_rows(&diff.packs));
    lines.extend(section("skills", &change_rows(&diff.skills)));
    lines.extend(section("mcps", &change_rows(&diff.mcps)));
    lines.extend(section("hooks", &change_rows(&diff.hooks)));
    lines
}

/// The whole bundle diff as one keyed record per namespace, each carrying its `+`/`~`/`-` counts.
pub fn diff_json(diff: &BundleDiff) -> JsonObject {
    let mut record = JsonObject::new();

    record.insert("hooks".to_owned(), change_json(&diff.hooks));
    record.insert("mcps".to_owned(), change_json(&diff.mcps));
    record.insert("packs".to_owned(), change_json(&diff.packs));
    record.insert("skills".to_owned(), change_json(&diff.skills));
    record
}
