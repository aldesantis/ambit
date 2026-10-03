//! YAML under ambit's rules: core schema 1.2, positioned errors, and a byte-stable emitter.
//!
//! Every document ambit reads goes through [`parse_yaml_mapping`] or
//! [`parse_frontmatter_mapping`], which build a positioned tree (`tree.rs`) from saphyr-parser
//! events, type plain scalars by the core schema (`schema.rs`), and reject what the TypeScript
//! build rejected: tabs as indentation, duplicate keys, non-string keys, custom tags, and more than
//! one document. [`YamlMapping`]'s accessors then either return a value of the requested type or
//! an error naming the key, the file, and the line.
//!
//! Writing goes through [`emit_yaml`] (`emit.rs`), kept in this module so the emit rules match the
//! parse rules: a string that would read back as anything else is double-quoted.

mod emit;
mod frontmatter;
mod schema;
mod tree;

use std::path::Path;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::errors::{AmbitError, Result};

pub use emit::emit_yaml;
pub use frontmatter::{FrontmatterSplit, split_frontmatter};

/// One string from a sequence, with where it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionedString {
    pub value: String,
    /// 1-based, absent when the document positioned neither the item nor its key.
    pub line: Option<usize>,
}

/// One item of a sequence that may hold a string or a mapping; see
/// [`YamlMapping::optional_entry_list`].
#[derive(Clone, Debug)]
pub enum YamlEntry {
    String(PositionedString),
    Mapping(YamlMapping),
}

/// A YAML mapping, with the accessors every ambit parser needs. Each one either returns a value of
/// the requested type or an [`AmbitError`] naming the key, the file, and the line.
///
/// A key that is present must carry a value: an explicit `null` is an error, because the way to
/// take a default is to omit the key.
#[derive(Clone, Debug)]
pub struct YamlMapping {
    document: Rc<tree::Document>,
    node: tree::NodeId,
    /// Dotted path from the document root, so nested errors read `catalogs[0].ref`.
    prefix: String,
}

impl YamlMapping {
    /// The file this mapping came from, as named in error messages.
    pub fn file(&self) -> &str {
        &self.document.file
    }

    /// The 1-based line this mapping starts on.
    pub fn line(&self) -> Option<usize> {
        let _ = (&self.node, &self.prefix);
        todo!("port model/yaml.ts:YamlMapping.line")
    }

    /// Keys in document order. Duplicates cannot occur: the loader rejects them.
    pub fn keys(&self) -> Vec<String> {
        todo!("port model/yaml.ts:YamlMapping.keys")
    }

    pub fn has(&self, key: &str) -> bool {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.has")
    }

    /// The line `key` appears on, falling back to the mapping's own line.
    pub fn line_of(&self, key: &str) -> Option<usize> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.lineOf")
    }

    /// Builds an error positioned at `key`, for rules only the caller knows.
    pub fn key_error(&self, key: &str, message: &str, detail: Vec<String>) -> AmbitError {
        let _ = (key, message, detail);
        todo!("port model/yaml.ts:YamlMapping.keyError")
    }

    /// Rejects any key outside `known`. Unknown keys are errors rather than warnings: a typo in an
    /// ignored key is indistinguishable from a feature that silently does nothing.
    ///
    /// # Errors
    ///
    /// Exit 2 naming the first unknown key.
    pub fn reject_unknown_keys(&self, known: &[&str]) -> Result<()> {
        let _ = known;
        todo!("port model/yaml.ts:YamlMapping.rejectUnknownKeys")
    }

    /// # Errors
    ///
    /// Exit 2 when the key is absent, null, or not a string.
    pub fn require_string(&self, key: &str) -> Result<String> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.requireString")
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null or not a string.
    pub fn optional_string(&self, key: &str) -> Result<Option<String>> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.optionalString")
    }

    /// # Errors
    ///
    /// Exit 2 when the key is absent, null, or not an integer.
    pub fn require_integer(&self, key: &str) -> Result<i64> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.requireInteger")
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null or not an integer.
    pub fn optional_integer(&self, key: &str) -> Result<Option<i64>> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.optionalInteger")
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null or not a boolean.
    pub fn optional_boolean(&self, key: &str) -> Result<Option<bool>> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.optionalBoolean")
    }

    /// A sequence of strings. An empty sequence is allowed and means exactly that.
    ///
    /// # Errors
    ///
    /// Exit 2 when the key is null, not a sequence, or holds a non-string.
    pub fn optional_string_list(&self, key: &str) -> Result<Option<Vec<String>>> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.optionalStringList")
    }

    /// The same sequence, each item paired with the line it was written on.
    ///
    /// A rule enforced after parsing (a `requires` pattern no catalog's items match, say) has no
    /// YAML node left to point at, but its error still needs to name a line.
    ///
    /// # Errors
    ///
    /// As [`YamlMapping::optional_string_list`].
    pub fn optional_positioned_string_list(
        &self,
        key: &str,
    ) -> Result<Option<Vec<PositionedString>>> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.optionalPositionedStringList")
    }

    /// # Errors
    ///
    /// Exit 2 when the key is absent, null, or not a mapping.
    pub fn require_mapping(&self, key: &str) -> Result<YamlMapping> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.requireMapping")
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null or not a mapping.
    pub fn optional_mapping(&self, key: &str) -> Result<Option<YamlMapping>> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.optionalMapping")
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null, not a sequence, or holds a non-mapping.
    pub fn optional_mapping_list(&self, key: &str) -> Result<Option<Vec<YamlMapping>>> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.optionalMappingList")
    }

    /// A sequence whose items are each a string or a mapping: the shape `requires` reads, where
    /// only the mapping form is legal.
    ///
    /// The string form is returned rather than rejected here so the caller can give a specific
    /// error: a bare pattern names neither the field it matches nor the capabilities it selects.
    /// Each string carries its line, because it may be judged long after parsing.
    ///
    /// # Errors
    ///
    /// Exit 2 when the key is null, not a sequence, or holds something else.
    pub fn optional_entry_list(&self, key: &str) -> Result<Option<Vec<YamlEntry>>> {
        let _ = key;
        todo!("port model/yaml.ts:YamlMapping.optionalEntryList")
    }

    /// Every entry of this mapping as string to string, for free-form maps whose keys are not known
    /// ahead of time (`transport.http.headers`).
    ///
    /// # Errors
    ///
    /// Exit 2 for a value that is not a string.
    pub fn string_entries(&self) -> Result<IndexMap<String, String>> {
        todo!("port model/yaml.ts:YamlMapping.stringEntries")
    }
}

/// Parses `text` as a YAML mapping under ambit's rules.
///
/// `file` is how the document is named in error messages: a project-relative path, not the
/// absolute one, since that is what the reader recognizes.
///
/// # Errors
///
/// Exit 2, naming the offending file, identifier, and line.
pub fn parse_yaml_mapping(text: &str, file: &str) -> Result<YamlMapping> {
    let _ = (text, file, schema::CORE_SCHEMA);
    todo!("port model/yaml.ts:parseYamlMapping")
}

/// Parses the frontmatter block of a Markdown document (`SKILL.md`'s, in practice) under the same
/// rules as a standalone YAML file.
///
/// Reported lines are lines of the whole document rather than of the extracted block, because a
/// reader told "line 4" must be able to go to line 4 of the file named.
///
/// # Errors
///
/// Exit 2 if there is no frontmatter, or it violates a rule.
pub fn parse_frontmatter_mapping(text: &str, file: &str) -> Result<YamlMapping> {
    let _ = (text, file);
    todo!("port model/yaml.ts:parseFrontmatterMapping")
}

/// Reads and parses a YAML file. `file` is how it is named in error messages.
///
/// # Errors
///
/// Exit 2 when the file cannot be read or does not parse.
pub fn read_yaml_mapping(path: &Path, file: &str) -> Result<YamlMapping> {
    let _ = (path, file);
    todo!("port model/yaml.ts:readYamlMapping")
}

/// Reads a Markdown file and parses its frontmatter block. `file` is how it is named in error
/// messages.
///
/// # Errors
///
/// Exit 2 when the file cannot be read or its frontmatter does not parse.
pub fn read_frontmatter_mapping(path: &Path, file: &str) -> Result<YamlMapping> {
    let _ = (path, file);
    todo!("port model/yaml.ts:readFrontmatterMapping")
}
