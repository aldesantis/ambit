//! The JSON driver: `.mcp.json`, `.cursor/mcp.json`, `.vscode/mcp.json`.
//!
//! JSON has no comments, so a parse round-trip loses nothing a person wrote and the driver can work
//! on parsed objects internally. Key order is preserved: the managed section stays where it already
//! was, instead of moving to the end on the first install.

use indexmap::IndexSet;

use crate::errors::Result;
use crate::model::documents::format::{ConfigEntry, DocumentDriver, DocumentFormat};
use crate::util::json::JsonObject;

/// Parses a JSON document, treating an absent file as an empty one.
///
/// Shared with the array-section driver (`json_array.rs`), which reads the same files in the same
/// syntax, so the two word their refusals identically.
///
/// # Errors
///
/// Exit 2 for malformed JSON or a non-object root, since overwriting either would destroy content
/// ambit does not own.
pub fn parse_json_document(text: Option<&str>, file: &str) -> Result<JsonObject> {
    let _ = (text, file);
    todo!("port model/documents/json.ts:parseJsonDocument")
}

/// Renders a document as the bytes written to disk: two-space indent, trailing newline.
pub fn serialize_json_document(document: &JsonObject) -> String {
    let _ = document;
    todo!("port model/documents/json.ts:serializeJsonDocument")
}

/// The map-shaped JSON driver.
#[derive(Clone, Copy, Debug, Default)]
pub struct JsonDriver;

impl DocumentDriver for JsonDriver {
    fn format(&self) -> DocumentFormat {
        DocumentFormat::Json
    }

    fn section_keys(
        &self,
        text: Option<&str>,
        section: &str,
        file: &str,
    ) -> Result<IndexSet<String>> {
        let _ = (text, section, file);
        todo!("port model/documents/json.ts:jsonDriver.sectionKeys")
    }

    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String> {
        let _ = (text, section, entries, file);
        todo!("port model/documents/json.ts:jsonDriver.mergeSection")
    }

    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        let _ = (text, section, entry, file);
        todo!("port model/documents/json.ts:jsonDriver.entryMatches")
    }

    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>> {
        let _ = (text, section, keys, file);
        todo!("port model/documents/json.ts:jsonDriver.removeKeys")
    }
}
