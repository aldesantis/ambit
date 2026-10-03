//! The YAML emitter: a hand port of the subset of yaml@2.9.0's `stringify` ambit uses.

use crate::util::json::JsonValue;

/// Renders a document as the bytes ambit writes: keys sorted at every depth, strings that would
/// otherwise coerce double-quoted, no anchors, no aliases, no rewrapping, and a trailing newline.
///
/// Byte-stable by construction: the output is a function of the values alone, so the same inputs
/// produce the same file and a lock that has not changed shows as no diff.
pub fn emit_yaml(document: &JsonValue) -> String {
    let _ = document;
    todo!("port model/yaml.ts:emitYaml")
}
