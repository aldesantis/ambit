//! The TOML driver: `.codex/config.toml`.
//!
//! This file is not ambit's. Codex keeps a person's model, sandbox, approval and profile settings
//! in it, often with comments. Parsing and re-stringifying through a TOML library would drop those
//! comments, so this driver never parses the document: it locates the `[mcp_servers.<name>]` table
//! for each server it owns and splices that span of lines, leaving every other byte identical.
//!
//! Some legal TOML has no replaceable span this way: a server declared as an inline table, or
//! through dotted keys outside a table header. Those are refused (exit 2, file untouched) rather
//! than guessed at, to avoid corrupting a config ambit was not asked to rewrite.

use indexmap::IndexSet;

use crate::errors::Result;
use crate::model::documents::format::{ConfigEntry, DocumentDriver, DocumentFormat};

/// The TOML driver.
#[derive(Clone, Copy, Debug, Default)]
pub struct TomlDriver;

impl DocumentDriver for TomlDriver {
    fn format(&self) -> DocumentFormat {
        DocumentFormat::Toml
    }

    fn section_keys(
        &self,
        text: Option<&str>,
        section: &str,
        file: &str,
    ) -> Result<IndexSet<String>> {
        let _ = (text, section, file);
        todo!("port model/documents/toml.ts:sectionKeys")
    }

    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String> {
        let _ = (text, section, entries, file);
        todo!("port model/documents/toml.ts:mergeSection")
    }

    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        let _ = (text, section, entry, file);
        todo!("port model/documents/toml.ts:entryMatches")
    }

    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>> {
        let _ = (text, section, keys, file);
        todo!("port model/documents/toml.ts:removeKeys")
    }
}
