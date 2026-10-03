//! The YAML emitter, on serde-saphyr.
//!
//! Keys are sorted at every depth before serializing, so the byte order is a function of the keys
//! rather than of whichever order a generator built its object in: the property that lets a lock
//! be diffed at all. Block scalars are turned off, so a long value stays on its line and an
//! awkward one is escaped inside double quotes rather than turned into a block whose indentation
//! carries meaning. A JSON value carries no shared references, so no anchors or aliases are
//! written.
//!
//! Quoting follows YAML 1.2, the schema ambit's loader reads, so a YAML 1.1 boolean spelling such
//! as `yes` or `off` stays plain. With that option serde-saphyr also opens the document with a
//! `%YAML 1.2` directive, which is cut off: ambit's files have never carried one.

use serde_saphyr::SerializerOptions;

use crate::util::cmp::js_cmp;
use crate::util::json::{JsonObject, JsonValue};

/// What serde-saphyr writes before the first node when `yaml_12` is on.
const YAML_12_HEADER: &str = "%YAML 1.2\n---\n";

/// Renders a document as the bytes ambit writes: keys sorted at every depth, strings that would
/// otherwise read back as another type quoted, no anchors, no block scalars, and a trailing
/// newline.
///
/// Byte-stable by construction: the output is a function of the values alone, so the same inputs
/// produce the same file and a lock that has not changed shows as no diff.
pub fn emit_yaml(document: &JsonValue) -> String {
    let mut options = SerializerOptions::default();

    options.prefer_block_scalars = false;
    options.compact_list_indent = false;
    options.yaml_12 = true;

    let text = serde_saphyr::to_string_with_options(&sorted(document), options)
        .expect("a JSON value always serializes to YAML");

    match text.strip_prefix(YAML_12_HEADER) {
        Some(body) => body.to_owned(),
        None => text,
    }
}

/// `value` with every mapping's keys in [`js_cmp`] order.
fn sorted(value: &JsonValue) -> JsonValue {
    match value {
        JsonValue::Array(items) => JsonValue::Array(items.iter().map(sorted).collect()),
        JsonValue::Object(object) => {
            let mut entries: Vec<(&String, &JsonValue)> = object.iter().collect();

            entries.sort_by(|a, b| js_cmp(a.0, b.0));

            JsonValue::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key.clone(), sorted(value)))
                    .collect::<JsonObject>(),
            )
        }
        scalar => scalar.clone(),
    }
}
