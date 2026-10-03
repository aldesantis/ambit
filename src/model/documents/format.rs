//! What every document driver shares: the formats, the section shapes, and the driver contract.

use std::path::Path;

use indexmap::IndexSet;

use crate::errors::Result;
use crate::util::json::JsonValue;
use crate::util::string_enum;

string_enum! {
    /// Which format a harness config file is written in.
    pub enum DocumentFormat {
        Json => "json",
        Jsonc => "jsonc",
        Toml => "toml",
    }
}

/// The formats a harness config file can be written in.
pub const DOCUMENT_FORMATS: &[DocumentFormat] = DocumentFormat::ALL;

string_enum! {
    /// The two shapes a managed section can have.
    ///
    /// `Map` is a table keyed by an entity's name (`mcpServers.<name>`), where identity is written
    /// in the document and a merge is a key assignment. `Array` is `<Event>: [entries]`, the shape
    /// every harness uses for hooks, where nothing in the document says which entry belongs to
    /// whom: identity is derived from the entry's content, and a merge is an append.
    ///
    /// Shape is not a property of format: `.mcp.json` and `.claude/settings.json` are both JSON, so
    /// format alone cannot pick a driver and both have to be carried.
    pub enum DocumentShape {
        Map => "map",
        Array => "array",
    }
}

/// Every section shape, in declaration order.
pub const DOCUMENT_SHAPES: &[DocumentShape] = DocumentShape::ALL;

/// One key ambit owns inside a section, and the value it writes there.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigEntry {
    pub key: String,
    /// The value, as plain JSON-compatible data. A driver renders it in its own syntax, so this
    /// must already be built in the order it should emit in.
    pub value: JsonValue,
}

/// Reads and writes one file format, preserving everything ambit does not own.
pub trait DocumentDriver {
    fn format(&self) -> DocumentFormat;

    /// The keys currently in the managed section: what ownership enforcement compares a plan
    /// against.
    ///
    /// An absent file, an absent section, or a section that is not a table of keys all read as
    /// empty: none is a collision with anything ambit would write. An unusable section is
    /// [`DocumentDriver::merge_section`]'s error to raise instead, since that is the code which
    /// cannot proceed with it.
    ///
    /// # Errors
    ///
    /// Exit 2 for a document that cannot be parsed.
    fn section_keys(
        &self,
        text: Option<&str>,
        section: &str,
        file: &str,
    ) -> Result<IndexSet<String>>;

    /// The file with `entries` merged into the managed section.
    ///
    /// Keys already present keep their position; only new ones are appended, so an install does
    /// not reorder lines ambit does not own.
    ///
    /// # Errors
    ///
    /// Exit 2 for a document, or a section, ambit cannot write into without discarding content.
    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String>;

    /// Whether one entry is already in the file as install would write it: `status`'s drift
    /// question.
    ///
    /// Each format answers in its own terms: where a document parses losslessly, key order and
    /// indentation are not differences; where it cannot, the bytes ambit would write are the only
    /// available answer.
    ///
    /// # Errors
    ///
    /// Exit 2 for a document that cannot be parsed.
    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool>;

    /// The file with `keys` removed from the managed section, or `None` when it held none of them.
    ///
    /// `None` lets a caller skip the write entirely: it keeps a prune with nothing stale
    /// byte-identical, and stops pruning from recreating a file someone deleted by hand.
    ///
    /// # Errors
    ///
    /// Exit 2 for a document that cannot be parsed.
    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>>;
}

/// Anything with keys: a JSON object, not an array or a scalar.
pub fn is_record(value: &JsonValue) -> bool {
    value.is_object()
}

/// Reads a config file, treating an absent one as no document at all.
///
/// `target` is the absolute path to read; `file` is how it is named in errors, conventionally
/// project-relative.
///
/// # Errors
///
/// Exit 2 when the file exists but cannot be read. "I could not look" is not the same answer as
/// "nothing is there"; treating it as the latter risks destroying data.
pub fn read_document_text(target: &Path, file: &str) -> Result<Option<String>> {
    let _ = (target, file);
    todo!("port model/documents/format.ts:readDocumentText")
}

/// The dotted key state records for one managed entry.
pub fn managed_key(section: &str, key: &str) -> String {
    format!("{section}.{key}")
}
