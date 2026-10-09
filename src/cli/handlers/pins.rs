use serde_json::json;

use crate::cli::output::{keyed, section};
use crate::project::bundle_diff::{BundleChange, BundleChangeKind, BundleDiff, count_changes};
use crate::project::update::CatalogPin;
use crate::util::json::{JsonObject, JsonValue};

const NO_COMMIT: &str = "-";

const ABBREVIATED: usize = 7;

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

pub fn diff_sections(diff: &BundleDiff) -> Vec<String> {
    let mut lines = section("packs", &change_rows(&diff.packs));
    lines.extend(section("skills", &change_rows(&diff.skills)));
    lines.extend(section("mcps", &change_rows(&diff.mcps)));
    lines.extend(section("hooks", &change_rows(&diff.hooks)));
    lines
}

pub fn diff_json(diff: &BundleDiff) -> JsonObject {
    let mut record = JsonObject::new();

    record.insert("hooks".to_owned(), change_json(&diff.hooks));
    record.insert("mcps".to_owned(), change_json(&diff.mcps));
    record.insert("packs".to_owned(), change_json(&diff.packs));
    record.insert("skills".to_owned(), change_json(&diff.skills));
    record
}
