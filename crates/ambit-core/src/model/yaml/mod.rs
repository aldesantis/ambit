//! The shared YAML loader.
//!
//! Every ambit format goes through here (`ambit.yml`, `mcps/*.yml`, `hook.yml`, `SKILL.md`
//! frontmatter), so parsing rules are enforced once and cannot drift between parsers. Without them
//! a commit SHA like `1234567` would parse as an integer, a duplicate key would quietly win, or a
//! tab could pass as indentation.
//!
//! Reading builds a positioned tree from saphyr-parser events (`tree.rs`), types plain scalars by
//! the YAML 1.2 core schema (`schema.rs`), and returns a [`YamlMapping`]: a positioned view over
//! the document rather than a plain value, so every downstream error can name the line it came
//! from.
//!
//! Writing goes through [`emit_yaml`] (`emit.rs`), kept in this module so the emit rules match the
//! parse rules: what ambit writes is guaranteed readable by what ambit reads.
//!
//! Editing a document someone else wrote goes through `edit.rs` instead, which splices the original
//! text so comments and formatting outside the edited nodes survive.

// Reached only through `config_edit`, which the CLI binary does not call.
#[allow(dead_code)]
pub(crate) mod edit;
mod emit;
mod frontmatter;
mod schema;
mod tree;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::rc::Rc;

use indexmap::IndexMap;

use crate::errors::{AmbitError, Result, at, config_error};
use crate::util::cmp::js_cmp;
use crate::util::fs::{io_message, read_text};
use crate::util::json::format_f64;
use crate::util::text::js_trim;

pub use emit::emit_yaml;
pub use frontmatter::split_frontmatter;

use schema::Scalar;
use tree::{NodeId, NodeKind, is_integer};

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
    ///
    /// Always present for a parsed mapping. Optional because it feeds [`crate::errors::at`], whose
    /// line is optional for values nothing positioned.
    #[allow(clippy::unnecessary_wraps)]
    pub fn line(&self) -> Option<usize> {
        Some(self.document.line_of(self.node))
    }

    /// Keys in document order. Duplicates cannot occur: the loader rejects them.
    pub fn keys(&self) -> Vec<String> {
        self.document
            .pairs(self.node)
            .iter()
            .filter_map(|&(key, _)| self.document.string(key).map(str::to_owned))
            .collect()
    }

    pub fn has(&self, key: &str) -> bool {
        self.pair_for(key).is_some()
    }

    /// The line `key` appears on, falling back to the mapping's own line.
    pub fn line_of(&self, key: &str) -> Option<usize> {
        self.pair_for(key)
            .map(|(key, _)| self.document.line_of(key))
            .or_else(|| self.line())
    }

    /// Builds an error positioned at `key`, for rules only the caller knows.
    pub fn key_error(&self, key: &str, message: &str, detail: Vec<String>) -> AmbitError {
        config_error(
            format!("{message} {}", at(self.file(), self.line_of(key))),
            detail,
        )
    }

    /// Rejects any key outside `known`. Unknown keys are errors rather than warnings: a typo in an
    /// ignored key is indistinguishable from a feature that silently does nothing.
    ///
    /// # Errors
    ///
    /// Exit 2 naming the first unknown key.
    pub fn reject_unknown_keys(&self, known: &[&str]) -> Result<()> {
        for key in self.keys() {
            if known.contains(&key.as_str()) {
                continue;
            }

            let mut accepted: Vec<&str> = Vec::new();

            for name in known {
                if !accepted.contains(name) {
                    accepted.push(name);
                }
            }

            accepted.sort_by(|a, b| js_cmp(a, b));

            return Err(self.key_error(
                &key,
                &format!("unknown key \"{}\"", self.label(&key)),
                vec![
                    format!("accepted keys: {}", accepted.join(", ")),
                    format!("remove `{key}`, or correct the spelling"),
                ],
            ));
        }

        Ok(())
    }

    /// # Errors
    ///
    /// Exit 2 when the key is absent, null, or not a string.
    pub fn require_string(&self, key: &str) -> Result<String> {
        let pair = self.require(key, "a string")?;

        self.read_string(pair, key, true)
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null or not a string.
    pub fn optional_string(&self, key: &str) -> Result<Option<String>> {
        self.pair_for(key)
            .map(|pair| self.read_string(pair, key, false))
            .transpose()
    }

    /// # Errors
    ///
    /// Exit 2 when the key is absent, null, or not an integer.
    // An integral f64 converts exactly up to 2^53, the range a JavaScript number held.
    #[allow(clippy::cast_possible_truncation)]
    pub fn require_integer(&self, key: &str) -> Result<i64> {
        let pair = self.require(key, "an integer")?;
        let value = self.value(pair, key, "an integer", true)?;

        match self.document.scalar(value) {
            Some(Scalar::Number(n)) if is_integer(*n) => Ok(*n as i64),
            _ => Err(self.mismatch(key, "an integer", value)),
        }
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null or not an integer.
    pub fn optional_integer(&self, key: &str) -> Result<Option<i64>> {
        if self.has(key) {
            self.require_integer(key).map(Some)
        } else {
            Ok(None)
        }
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null or not a boolean.
    pub fn optional_boolean(&self, key: &str) -> Result<Option<bool>> {
        if !self.has(key) {
            return Ok(None);
        }

        let pair = self.require(key, "a boolean")?;
        let value = self.value(pair, key, "a boolean", true)?;

        match self.document.scalar(value) {
            Some(Scalar::Bool(b)) => Ok(Some(*b)),
            _ => Err(self.mismatch(key, "a boolean", value)),
        }
    }

    /// A sequence of strings. An empty sequence is allowed and means exactly that.
    ///
    /// # Errors
    ///
    /// Exit 2 when the key is null, not a sequence, or holds a non-string.
    pub fn optional_string_list(&self, key: &str) -> Result<Option<Vec<String>>> {
        Ok(self
            .optional_positioned_string_list(key)?
            .map(|entries| entries.into_iter().map(|entry| entry.value).collect()))
    }

    /// The same sequence, each item paired with the line it was written on.
    ///
    /// A rule enforced after parsing (a `requires` pattern no catalog's items match, say) has no
    /// YAML node left to point at, but its error still needs to name a line. Carrying the positions
    /// forward avoids reparsing the document to find them again.
    ///
    /// # Errors
    ///
    /// As [`YamlMapping::optional_string_list`].
    pub fn optional_positioned_string_list(
        &self,
        key: &str,
    ) -> Result<Option<Vec<PositionedString>>> {
        let Some(items) = self.sequence(key, "a sequence of strings")? else {
            return Ok(None);
        };

        items
            .iter()
            .enumerate()
            .map(|(index, &item)| {
                let value = self.read_item_string(item, key, index, "a string")?;
                let line = Some(self.document.line_of(item));

                Ok(PositionedString { value, line })
            })
            .collect::<Result<Vec<_>>>()
            .map(Some)
    }

    /// # Errors
    ///
    /// Exit 2 when the key is absent, null, or not a mapping.
    pub fn require_mapping(&self, key: &str) -> Result<YamlMapping> {
        let pair = self.require(key, "a mapping")?;
        let value = self.value(pair, key, "a mapping", true)?;

        if !self.document.is_map(value) {
            return Err(self.mismatch(key, "a mapping", value));
        }

        Ok(self.child(value, self.label(key)))
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null or not a mapping.
    pub fn optional_mapping(&self, key: &str) -> Result<Option<YamlMapping>> {
        if self.has(key) {
            self.require_mapping(key).map(Some)
        } else {
            Ok(None)
        }
    }

    /// # Errors
    ///
    /// Exit 2 when the key is null, not a sequence, or holds a non-mapping.
    pub fn optional_mapping_list(&self, key: &str) -> Result<Option<Vec<YamlMapping>>> {
        let Some(items) = self.sequence(key, "a sequence of mappings")? else {
            return Ok(None);
        };

        items
            .iter()
            .enumerate()
            .map(|(index, &item)| {
                if !self.document.is_map(item) {
                    return Err(self.item_mismatch(key, index, "a mapping", item));
                }

                Ok(self.child(item, format!("{}[{index}]", self.label(key))))
            })
            .collect::<Result<Vec<_>>>()
            .map(Some)
    }

    /// A sequence whose items are each a string or a mapping: the shape `requires` reads, where
    /// only the mapping form is legal.
    ///
    /// The string form is returned rather than rejected here so the caller can give a specific
    /// error: a bare pattern names neither the field it matches nor the capabilities it selects,
    /// and a generic "must be a mapping" message would not say that.
    ///
    /// Each string carries its line, like [`YamlMapping::optional_positioned_string_list`],
    /// because it may be judged long after parsing and its error must still name the line it was
    /// written on.
    ///
    /// # Errors
    ///
    /// Exit 2 when the key is null, not a sequence, or holds something else.
    pub fn optional_entry_list(&self, key: &str) -> Result<Option<Vec<YamlEntry>>> {
        let Some(items) = self.sequence(key, "a sequence of strings or mappings")? else {
            return Ok(None);
        };

        items
            .iter()
            .enumerate()
            .map(|(index, &item)| {
                if self.document.is_map(item) {
                    return Ok(YamlEntry::Mapping(
                        self.child(item, format!("{}[{index}]", self.label(key))),
                    ));
                }

                let value = self.read_item_string(item, key, index, "a string or a mapping")?;
                let line = Some(self.document.line_of(item));

                Ok(YamlEntry::String(PositionedString { value, line }))
            })
            .collect::<Result<Vec<_>>>()
            .map(Some)
    }

    /// Every entry of this mapping as string to string, for free-form maps whose keys are not known
    /// ahead of time (`transport.http.headers`).
    ///
    /// # Errors
    ///
    /// Exit 2 for a value that is not a string.
    pub fn string_entries(&self) -> Result<IndexMap<String, String>> {
        let mut entries = IndexMap::new();

        for key in self.keys() {
            let value = self.require_string(&key)?;

            entries.insert(key, value);
        }

        Ok(entries)
    }

    fn child(&self, node: NodeId, prefix: String) -> YamlMapping {
        YamlMapping {
            document: Rc::clone(&self.document),
            node,
            prefix,
        }
    }

    fn label(&self, key: &str) -> String {
        if self.prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{}.{key}", self.prefix)
        }
    }

    /// The `(key, value)` nodes of the pair whose key is the string `key`.
    fn pair_for(&self, key: &str) -> Option<(NodeId, NodeId)> {
        self.document
            .pairs(self.node)
            .iter()
            .copied()
            .find(|&(name, _)| self.document.string(name) == Some(key))
    }

    fn require(&self, key: &str, expected: &str) -> Result<(NodeId, NodeId)> {
        self.pair_for(key).ok_or_else(|| {
            config_error(
                format!(
                    "missing required key \"{}\" {}",
                    self.label(key),
                    at(self.file(), self.line())
                ),
                [
                    format!("expected {expected}"),
                    format!("add `{key}:` with a value"),
                ],
            )
        })
    }

    /// The value node behind `key`, rejecting an explicit null: the way to take a default is to
    /// omit the key, so a written-out `null` is always a mistake.
    fn value(
        &self,
        (_, value): (NodeId, NodeId),
        key: &str,
        expected: &str,
        required: bool,
    ) -> Result<NodeId> {
        if self.document.scalar(value) == Some(&Scalar::Null) {
            return Err(self.key_error(
                key,
                &format!("\"{}\" must not be null", self.label(key)),
                vec![
                    format!("expected {expected}"),
                    if required {
                        "give it a value".to_owned()
                    } else {
                        "give it a value, or remove the key to take its default".to_owned()
                    },
                ],
            ));
        }

        Ok(value)
    }

    fn read_string(&self, pair: (NodeId, NodeId), key: &str, required: bool) -> Result<String> {
        let value = self.value(pair, key, "a string", required)?;

        self.coerce_string(value, &self.label(key), self.line_of(key), "a string")
    }

    fn read_item_string(
        &self,
        item: NodeId,
        key: &str,
        index: usize,
        expected: &str,
    ) -> Result<String> {
        let label = format!("{}[{index}]", self.label(key));

        if self.document.scalar(item) == Some(&Scalar::Null) {
            return Err(config_error(
                format!(
                    "\"{label}\" must not be null {}",
                    at(self.file(), Some(self.document.line_of(item)))
                ),
                [
                    format!("expected {expected}"),
                    "give it a value, or remove the entry".to_owned(),
                ],
            ));
        }

        let line = Some(self.document.line_of(item));

        self.coerce_string(item, &label, line, expected)
    }

    /// Enforces the central rule: anything that identifies something must arrive as a string. A
    /// number or boolean is reported rather than stringified, because silently accepting
    /// `ref: 1e5` as `"100000"` is how a config comes to point at the wrong commit.
    fn coerce_string(
        &self,
        value: NodeId,
        label: &str,
        line: Option<usize>,
        expected: &str,
    ) -> Result<String> {
        let file = self.file();

        match self.document.scalar(value) {
            Some(Scalar::String(text)) => {
                if js_trim(text).is_empty() {
                    return Err(config_error(
                        format!("\"{label}\" must not be empty {}", at(file, line)),
                        [
                            format!("expected {expected}"),
                            "give it a value, or remove the key".to_owned(),
                        ],
                    ));
                }

                Ok(text.clone())
            }
            Some(scalar @ (Scalar::Number(_) | Scalar::Bool(_))) => {
                let (shown, kind) = match scalar {
                    Scalar::Bool(b) => (b.to_string(), "a boolean"),
                    Scalar::Number(n) => (number_string(*n), "a number"),
                    _ => unreachable!("matched a number or a boolean"),
                };
                let written = self.document.text_of(value).map_or(shown, str::to_owned);

                Err(config_error(
                    format!("\"{label}\" must be a string {}", at(file, line)),
                    [
                        format!("YAML parsed `{written}` as {kind}"),
                        format!("quote it: `{}`", quote_hint(label, &written)),
                    ],
                ))
            }
            _ => Err(config_error(
                format!("\"{label}\" must be {expected} {}", at(file, line)),
                [
                    format!("found {}", self.document.describe(value)),
                    format!("give `{label}` {expected}"),
                ],
            )),
        }
    }

    /// The items of an optional sequence-valued key, or `None` when the key is absent.
    fn sequence(&self, key: &str, expected: &str) -> Result<Option<Vec<NodeId>>> {
        let Some(pair) = self.pair_for(key) else {
            return Ok(None);
        };

        let value = self.value(pair, key, expected, false)?;

        match self.document.kind(value) {
            NodeKind::Seq(items) => Ok(Some(items.clone())),
            _ => Err(self.mismatch(key, expected, value)),
        }
    }

    fn mismatch(&self, key: &str, expected: &str, value: NodeId) -> AmbitError {
        self.key_error(
            key,
            &format!("\"{}\" must be {expected}", self.label(key)),
            vec![
                format!("found {}", self.document.describe(value)),
                format!("give `{key}` {expected}"),
            ],
        )
    }

    fn item_mismatch(&self, key: &str, index: usize, expected: &str, value: NodeId) -> AmbitError {
        let label = format!("{}[{index}]", self.label(key));
        let line = Some(self.document.line_of(value));

        config_error(
            format!("\"{label}\" must be {expected} {}", at(self.file(), line)),
            [
                format!("found {}", self.document.describe(value)),
                format!("give every `{key}` entry {expected}"),
            ],
        )
    }
}

/// `String(n)` for a number, including the non-finite values `format_f64` leaves to JSON.
fn number_string(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_owned()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else {
        format_f64(n)
    }
}

/// The fix to suggest for an unquoted value: `ref: "1e5"` for a key, `- "1e5"` for a sequence
/// item, so the suggestion is something the reader can paste back.
fn quote_hint(label: &str, written: &str) -> String {
    if label.ends_with(']') {
        return format!("- \"{written}\"");
    }

    let key = label.rfind('.').map_or(label, |dot| &label[dot + 1..]);

    format!("{key}: \"{written}\"")
}

/// Parses `text` and enforces every rule on it, keeping the parsed tree rather than reducing it to
/// a plain value, so callers can read node positions off it.
///
/// `line_offset` is the number of lines of the containing file above `text`, for a frontmatter
/// block.
fn parse_checked(text: &str, file: &str, line_offset: usize) -> Result<YamlMapping> {
    let document = tree::parse(text, file, line_offset)?;

    if let Some(problem) = tree::structural_problem(&document) {
        return Err(problem);
    }

    let Some(root) = document.root else {
        return Err(config_error(
            format!("{file} is empty"),
            [
                "expected a YAML mapping",
                "add the keys this format requires",
            ],
        ));
    };

    if !document.is_map(root) {
        return Err(config_error(
            format!(
                "root is not a mapping {}",
                at(file, Some(document.line_of(root)))
            ),
            [
                format!("found {} at the document root", document.describe(root)),
                "write the document as `key: value` pairs".to_owned(),
            ],
        ));
    }

    Ok(YamlMapping {
        document: Rc::new(document),
        node: root,
        prefix: String::new(),
    })
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
    parse_checked(text, file, 0)
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
    let split = split_frontmatter(text, file)?;

    parse_checked(&split.block, file, line_count(&split.open))
}

/// How many lines `text` occupies above whatever follows it.
fn line_count(text: &str) -> usize {
    text.matches('\n').count()
}

fn read_source(path: &Path, file: &str) -> Result<String> {
    read_text(path).map_err(|error| {
        config_error(
            format!("cannot read {file}"),
            [
                io_message(&error, "open", path),
                "check the path and its permissions".to_owned(),
            ],
        )
    })
}

/// Reads and parses a YAML file. `file` is how it is named in error messages.
///
/// # Errors
///
/// Exit 2 when the file cannot be read or does not parse.
pub fn read_yaml_mapping(path: &Path, file: &str) -> Result<YamlMapping> {
    parse_yaml_mapping(&read_source(path, file)?, file)
}

/// Reads a Markdown file and parses its frontmatter block. `file` is how it is named in error
/// messages.
///
/// # Errors
///
/// Exit 2 when the file cannot be read or its frontmatter does not parse.
pub fn read_frontmatter_mapping(path: &Path, file: &str) -> Result<YamlMapping> {
    parse_frontmatter_mapping(&read_source(path, file)?, file)
}
