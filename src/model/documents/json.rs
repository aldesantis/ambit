//! The JSON driver: `.mcp.json`, `.cursor/mcp.json`, `.vscode/mcp.json`.
//!
//! JSON has no comments, so a parse round-trip loses nothing a person wrote and the driver can work
//! on parsed objects internally. Key order is preserved: every foreign key stays in place and the
//! managed section stays where it already was, instead of moving to the end on the first install.

use indexmap::IndexSet;

use crate::errors::{Result, config_error};
use crate::model::documents::format::{ConfigEntry, DocumentDriver};
use crate::util::json::{self, JsonObject, JsonValue, structurally_equal};

/// Parses a JSON document, treating an absent file as an empty one.
///
/// Shared with the array-section driver (`json_array.rs`), which reads the same files in the same
/// syntax and differs only in what it does with one section of them, so the two share this and
/// [`serialize_json_document`] instead of each having its own idea of what a JSON document is.
/// That keeps their refusals worded identically.
///
/// # Errors
///
/// Exit 2 for malformed JSON or a non-object root, since overwriting either would destroy content
/// ambit does not own.
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

/// The managed section as an object; anything unusable reads as absent.
pub(super) fn section_of<'a>(document: &'a JsonObject, section: &str) -> Option<&'a JsonObject> {
    document.get(section).and_then(JsonValue::as_object)
}

/// Renders a document as the bytes written to disk: two-space indent, trailing newline.
pub fn serialize_json_document(document: &JsonObject) -> String {
    format!(
        "{}\n",
        json::stringify_pretty(&JsonValue::Object(document.clone()))
    )
}

/// The map-shaped JSON driver.
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

        // An existing section keeps its position; a new one is appended.
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

    /// The section survives emptying out. ambit owns keys inside this file, not the file itself,
    /// so removing the last managed server leaves `{}` behind instead of deleting a document a
    /// person may also be writing into.
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
