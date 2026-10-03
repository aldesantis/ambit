//! The JSONC driver: `.opencode/opencode.jsonc`.
//!
//! JSONC exists so that a person can annotate their config, so comments, trailing commas,
//! indentation and key order everywhere ambit does not own survive being written into. Edits go
//! through jsonc-parser's CST; reads through its serde conversion, so a reformatted entry is not
//! drift.

use indexmap::IndexSet;

use crate::errors::Result;
use crate::model::documents::format::{ConfigEntry, DocumentDriver, DocumentFormat};

/// The JSONC driver.
#[derive(Clone, Copy, Debug, Default)]
pub struct JsoncDriver;

impl DocumentDriver for JsoncDriver {
    fn format(&self) -> DocumentFormat {
        DocumentFormat::Jsonc
    }

    fn section_keys(
        &self,
        text: Option<&str>,
        section: &str,
        file: &str,
    ) -> Result<IndexSet<String>> {
        let _ = (text, section, file);
        todo!("port model/documents/jsonc.ts:sectionKeys")
    }

    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String> {
        let _ = (text, section, entries, file);
        todo!("port model/documents/jsonc.ts:mergeSection")
    }

    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        let _ = (text, section, entry, file);
        todo!("port model/documents/jsonc.ts:entryMatches")
    }

    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>> {
        let _ = (text, section, keys, file);
        todo!("port model/documents/jsonc.ts:removeKeys")
    }
}
