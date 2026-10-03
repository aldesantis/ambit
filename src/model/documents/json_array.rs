//! The array-section driver: `.claude/settings.json`, `.cursor/hooks.json`, `.codex/hooks.json`.
//!
//! Every harness writes hooks as `<Event>: [entries]`. An array has no identity key: nothing in
//! `[{"matcher": "Bash", "hooks": [...]}, ...]` says which entry a tool wrote and which a person
//! did. So hooks cannot be merged by key the way MCP servers are.
//!
//! This driver makes the content the identity: a managed key is `<Event>@<digest>`, where the
//! digest is the first [`DIGEST_LENGTH`] hex characters of the SHA-256 of the entry ambit writes.
//!
//! - The key is derivable from the file alone, so `section_keys` needs no name the document does
//!   not carry, and `hooks.PostToolUse@a1b2c3` is an ordinary `<section>.<key>` pair like any
//!   other.
//! - An entry with any other digest is not a key ambit ever plans, so ownership never looks at it,
//!   pruning never names it, and a merge leaves it byte-identical. Hooks added outside ambit
//!   survive installs as a result of this identity scheme, not as a separate rule.
//!
//! Parsing and serialization are shared with the map-shaped JSON driver (`json.rs`).

use indexmap::IndexSet;

use crate::errors::Result;
use crate::model::documents::format::{ConfigEntry, DocumentDriver, DocumentFormat};
use crate::util::json::{JsonObject, JsonValue};

/// How much of the SHA-256 a key carries: 48 bits.
///
/// A file holds only a handful of hooks, so collision risk is not worth widening a key that a
/// person reads in `.ambit/state.json` and in every `status` row.
pub const DIGEST_LENGTH: usize = 12;

/// The digest of one entry as ambit would write it: `sha256(util::json::stringify(value))`,
/// truncated to [`DIGEST_LENGTH`].
///
/// Canonical JSON here means no whitespace, with keys kept in the order [`ConfigEntry`] already
/// promises. Sorting keys would make the digest describe something other than the bytes on disk.
///
/// Nothing outside the entry feeds the digest: no timestamps, no absolute paths, no environment.
/// Two people installing the same bundle get the same digests, which the lock relies on.
pub fn entry_digest(value: &JsonValue) -> String {
    let _ = value;
    todo!("port model/documents/json-array.ts:entryDigest")
}

/// The key within the section that names one entry: `<Event>@<digest>`.
pub fn array_entry_key(event: &str, value: &JsonValue) -> String {
    let _ = (event, value);
    todo!("port model/documents/json-array.ts:arrayEntryKey")
}

/// The array-section driver, with the root keys it seeds on a merge.
#[derive(Clone, Debug, Default)]
pub struct ArraySectionDriver {
    /// Root keys to seed, e.g. Cursor's `version: 1`. Written only where the document does not
    /// already have the key. A removal applies none of them: pruning must not add keys.
    pub root_defaults: JsonObject,
}

/// Builds the driver. `None` seeds nothing.
pub fn array_section_driver(root_defaults: Option<&JsonObject>) -> ArraySectionDriver {
    ArraySectionDriver {
        root_defaults: root_defaults.cloned().unwrap_or_default(),
    }
}

impl DocumentDriver for ArraySectionDriver {
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
        todo!("port model/documents/json-array.ts:sectionKeys")
    }

    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String> {
        let _ = (text, section, entries, file);
        todo!("port model/documents/json-array.ts:mergeSection")
    }

    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        let _ = (text, section, entry, file);
        todo!("port model/documents/json-array.ts:entryMatches")
    }

    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>> {
        let _ = (text, section, keys, file);
        todo!("port model/documents/json-array.ts:removeKeys")
    }
}
