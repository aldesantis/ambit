use serde_saphyr::SerializerOptions;

use crate::util::cmp::js_cmp;
use crate::util::json::{JsonObject, JsonValue};

const YAML_12_HEADER: &str = "%YAML 1.2\n---\n";

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
