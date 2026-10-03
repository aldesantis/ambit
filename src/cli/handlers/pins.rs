//! How `outdated` and `update` render where a pin stands and what moving it changes.
//!
//! The commit column is abbreviated: a full pair of forty-hex strings pushes everything worth
//! reading off the line, and `ambit.lock` already holds the exact commits.

use crate::project::bundle_diff::BundleDiff;
use crate::project::update::CatalogPin;
use crate::util::json::JsonObject;

/// One row per configured catalog: its name, where it stands, and the commit or the move.
pub fn pin_rows(pins: &[CatalogPin]) -> Vec<Vec<String>> {
    let _ = pins;
    todo!("port cli/handlers/pins.ts:pinRows")
}

/// Every pin as a name-keyed JSON record, with full commits.
pub fn pin_json(pins: &[CatalogPin]) -> JsonObject {
    let _ = pins;
    todo!("port cli/handlers/pins.ts:pinJson")
}

/// The whole bundle diff as one section per namespace, in the order every other report lists them.
pub fn diff_sections(diff: &BundleDiff) -> Vec<String> {
    let _ = diff;
    todo!("port cli/handlers/pins.ts:diffSections")
}

/// The whole bundle diff as one keyed record per namespace, each carrying its `+`/`~`/`-` counts.
pub fn diff_json(diff: &BundleDiff) -> JsonObject {
    let _ = diff;
    todo!("port cli/handlers/pins.ts:diffJson")
}
