use indexmap::IndexSet;

use crate::errors::{Result, config_error};
use crate::model::documents::format::{ConfigEntry, DocumentDriver};
use crate::util::json::{self, JsonObject, JsonValue, structurally_equal};

pub fn parse_json_document(text: Option<&str>, file: &str) -> Result<JsonObject> {
    let Some(text) = text else {
        return Ok(JsonObject::new());
    };

    let document = json::parse(text).map_err(|error| {
        config_error(
            format!("{file} is not valid JSON"),
            [
                error.to_string(),
                "correct the syntax, so ambit can add its own keys without discarding the rest"
                    .to_owned(),
            ],
        )
    })?;

    match document {
        JsonValue::Object(document) => Ok(document),
        _ => Err(config_error(
            format!("{file} is not a JSON object"),
            [
                "ambit merges its keys into this document, which requires an object at the root",
                "make the document an object, or move the file aside",
            ],
        )),
    }
}

pub(super) fn section_of<'a>(document: &'a JsonObject, section: &str) -> Option<&'a JsonObject> {
    document.get(section).and_then(JsonValue::as_object)
}

pub fn serialize_json_document(document: &JsonObject) -> String {
    format!(
        "{}\n",
        json::stringify_pretty(&JsonValue::Object(document.clone()))
    )
}

#[derive(Clone, Copy, Debug, Default)]
pub struct JsonDriver;

impl DocumentDriver for JsonDriver {
    fn section_keys(
        &self,
        text: Option<&str>,
        section: &str,
        file: &str,
    ) -> Result<IndexSet<String>> {
        let document = parse_json_document(text, file)?;

        Ok(section_of(&document, section)
            .map(|existing| existing.keys().cloned().collect())
            .unwrap_or_default())
    }

    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String> {
        let mut document = parse_json_document(text, file)?;

        let mut merged = match document.get(section) {
            None => JsonObject::new(),
            Some(JsonValue::Object(existing)) => existing.clone(),
            Some(_) => {
                return Err(config_error(
                    format!("\"{section}\" in {file} is not a JSON object"),
                    [
                        format!("ambit writes one key per managed entry inside `{section}`"),
                        format!("make `{section}` an object, or move its current value aside"),
                    ],
                ));
            }
        };

        for entry in entries {
            merged.insert(entry.key.clone(), entry.value.clone());
        }

        document.insert(section.to_owned(), JsonValue::Object(merged));

        Ok(serialize_json_document(&document))
    }

    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        let document = parse_json_document(text, file)?;

        Ok(section_of(&document, section)
            .and_then(|existing| existing.get(&entry.key))
            .is_some_and(|actual| structurally_equal(&entry.value, actual)))
    }

    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>> {
        let mut document = parse_json_document(text, file)?;

        let Some(existing) = section_of(&document, section) else {
            return Ok(None);
        };

        if !keys.iter().any(|key| existing.contains_key(key)) {
            return Ok(None);
        }

        let mut kept = existing.clone();

        for key in keys {
            kept.shift_remove(key);
        }

        document.insert(section.to_owned(), JsonValue::Object(kept));

        Ok(Some(serialize_json_document(&document)))
    }
}
