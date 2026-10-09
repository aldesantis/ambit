mod emit;
mod frontmatter;
mod load;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::rc::Rc;

use indexmap::IndexMap;
use saphyr::ScalarOwned;

use crate::errors::{AmbitError, Result, at, config_error};
use crate::util::cmp::js_cmp;
use crate::util::fs::{io_message, read_text};
use crate::util::json::format_f64;
use crate::util::text::js_trim;

pub use emit::emit_yaml;

use load::{Document, Node, is_integer};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PositionedString {
    pub value: String,
    pub line: Option<usize>,
}

#[derive(Clone, Debug)]
pub enum YamlEntry {
    String(PositionedString),
    Mapping(YamlMapping),
}

#[derive(Clone, Debug)]
pub struct YamlMapping {
    document: Rc<Document>,
    node: Rc<Node>,
    prefix: String,
}

impl YamlMapping {
    pub fn file(&self) -> &str {
        &self.document.file
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn line(&self) -> Option<usize> {
        Some(self.document.line_of(&self.node))
    }

    pub fn keys(&self) -> Vec<String> {
        self.document
            .pairs(&self.node)
            .into_iter()
            .filter_map(|(key, _)| self.document.string(key).map(str::to_owned))
            .collect()
    }

    pub fn has(&self, key: &str) -> bool {
        self.pair_for(key).is_some()
    }

    pub fn line_of(&self, key: &str) -> Option<usize> {
        self.pair_for(key)
            .map(|(key, _)| self.document.line_of(key))
            .or_else(|| self.line())
    }

    pub fn key_error(&self, key: &str, message: &str, detail: Vec<String>) -> AmbitError {
        config_error(
            format!("{message} {}", at(self.file(), self.line_of(key))),
            detail,
        )
    }

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

    pub fn require_string(&self, key: &str) -> Result<String> {
        let pair = self.require(key, "a string")?;

        self.read_string(pair, key, true)
    }

    pub fn optional_string(&self, key: &str) -> Result<Option<String>> {
        self.pair_for(key)
            .map(|pair| self.read_string(pair, key, false))
            .transpose()
    }

    #[allow(clippy::cast_possible_truncation)]
    pub fn require_integer(&self, key: &str) -> Result<i64> {
        let pair = self.require(key, "an integer")?;
        let value = self.value(pair, key, "an integer", true)?;

        match self.document.scalar(value) {
            Some(ScalarOwned::Integer(n)) => Ok(*n),
            Some(ScalarOwned::FloatingPoint(n)) if is_integer(n.0) => Ok(n.0 as i64),
            _ => Err(self.mismatch(key, "an integer", value)),
        }
    }

    pub fn optional_integer(&self, key: &str) -> Result<Option<i64>> {
        if self.has(key) {
            self.require_integer(key).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn optional_boolean(&self, key: &str) -> Result<Option<bool>> {
        if !self.has(key) {
            return Ok(None);
        }

        let pair = self.require(key, "a boolean")?;
        let value = self.value(pair, key, "a boolean", true)?;

        match self.document.scalar(value) {
            Some(ScalarOwned::Boolean(b)) => Ok(Some(*b)),
            _ => Err(self.mismatch(key, "a boolean", value)),
        }
    }

    pub fn optional_string_list(&self, key: &str) -> Result<Option<Vec<String>>> {
        Ok(self
            .optional_positioned_string_list(key)?
            .map(|entries| entries.into_iter().map(|entry| entry.value).collect()))
    }

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
            .map(|(index, item)| {
                let value = self.read_item_string(item, key, index, "a string")?;
                let line = Some(self.document.line_of(item));

                Ok(PositionedString { value, line })
            })
            .collect::<Result<Vec<_>>>()
            .map(Some)
    }

    pub fn require_mapping(&self, key: &str) -> Result<YamlMapping> {
        let pair = self.require(key, "a mapping")?;
        let value = self.value(pair, key, "a mapping", true)?;

        if !self.document.is_map(value) {
            return Err(self.mismatch(key, "a mapping", value));
        }

        Ok(self.child(value, self.label(key)))
    }

    pub fn optional_mapping(&self, key: &str) -> Result<Option<YamlMapping>> {
        if self.has(key) {
            self.require_mapping(key).map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn optional_mapping_list(&self, key: &str) -> Result<Option<Vec<YamlMapping>>> {
        let Some(items) = self.sequence(key, "a sequence of mappings")? else {
            return Ok(None);
        };

        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                if !self.document.is_map(item) {
                    return Err(self.item_mismatch(key, index, "a mapping", item));
                }

                Ok(self.child(item, format!("{}[{index}]", self.label(key))))
            })
            .collect::<Result<Vec<_>>>()
            .map(Some)
    }

    pub fn optional_entry_list(&self, key: &str) -> Result<Option<Vec<YamlEntry>>> {
        let Some(items) = self.sequence(key, "a sequence of strings or mappings")? else {
            return Ok(None);
        };

        items
            .iter()
            .enumerate()
            .map(|(index, item)| {
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

    pub fn string_entries(&self) -> Result<IndexMap<String, String>> {
        let mut entries = IndexMap::new();

        for key in self.keys() {
            let value = self.require_string(&key)?;

            entries.insert(key, value);
        }

        Ok(entries)
    }

    fn child(&self, node: &Node, prefix: String) -> YamlMapping {
        YamlMapping {
            document: Rc::clone(&self.document),
            node: Rc::new(node.clone()),
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

    fn pair_for(&self, key: &str) -> Option<(&Node, &Node)> {
        self.document
            .pairs(&self.node)
            .into_iter()
            .find(|&(name, _)| self.document.string(name) == Some(key))
    }

    fn require(&self, key: &str, expected: &str) -> Result<(&Node, &Node)> {
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

    fn value<'n>(
        &self,
        (_, value): (&'n Node, &'n Node),
        key: &str,
        expected: &str,
        required: bool,
    ) -> Result<&'n Node> {
        if self.document.is_null(value) {
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

    fn read_string(&self, pair: (&Node, &Node), key: &str, required: bool) -> Result<String> {
        let value = self.value(pair, key, "a string", required)?;

        self.coerce_string(value, &self.label(key), self.line_of(key), "a string")
    }

    fn read_item_string(
        &self,
        item: &Node,
        key: &str,
        index: usize,
        expected: &str,
    ) -> Result<String> {
        let label = format!("{}[{index}]", self.label(key));

        if self.document.is_null(item) {
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

    fn coerce_string(
        &self,
        value: &Node,
        label: &str,
        line: Option<usize>,
        expected: &str,
    ) -> Result<String> {
        let file = self.file();

        match self.document.scalar(value) {
            Some(ScalarOwned::String(text)) => {
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
            Some(
                scalar @ (ScalarOwned::Integer(_)
                | ScalarOwned::FloatingPoint(_)
                | ScalarOwned::Boolean(_)),
            ) => {
                let (shown, kind) = match scalar {
                    ScalarOwned::Boolean(b) => (b.to_string(), "a boolean"),
                    ScalarOwned::Integer(n) => (n.to_string(), "a number"),
                    ScalarOwned::FloatingPoint(n) => (number_string(n.0), "a number"),
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

    fn sequence(&self, key: &str, expected: &str) -> Result<Option<&[Node]>> {
        let Some(pair) = self.pair_for(key) else {
            return Ok(None);
        };

        let value = self.value(pair, key, expected, false)?;

        match self.document.items(value) {
            Some(items) => Ok(Some(items)),
            None => Err(self.mismatch(key, expected, value)),
        }
    }

    fn mismatch(&self, key: &str, expected: &str, value: &Node) -> AmbitError {
        self.key_error(
            key,
            &format!("\"{}\" must be {expected}", self.label(key)),
            vec![
                format!("found {}", self.document.describe(value)),
                format!("give `{key}` {expected}"),
            ],
        )
    }

    fn item_mismatch(&self, key: &str, index: usize, expected: &str, value: &Node) -> AmbitError {
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

fn number_string(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_owned()
    } else if n.is_infinite() {
        if n > 0.0 { "Infinity" } else { "-Infinity" }.to_owned()
    } else {
        format_f64(n)
    }
}

fn quote_hint(label: &str, written: &str) -> String {
    if label.ends_with(']') {
        return format!("- \"{written}\"");
    }

    let key = label.rfind('.').map_or(label, |dot| &label[dot + 1..]);

    format!("{key}: \"{written}\"")
}

fn parse_checked(
    text: &str,
    file: &str,
    line_offset: usize,
    empty: impl FnOnce() -> AmbitError,
) -> Result<YamlMapping> {
    let mut document = load::parse(text, file, line_offset)?;

    let Some(root) = document.root.take() else {
        return Err(empty());
    };

    if !document.is_map(&root) {
        return Err(config_error(
            format!(
                "root is not a mapping {}",
                at(file, Some(document.line_of(&root)))
            ),
            [
                format!("found {} at the document root", document.describe(&root)),
                "write the document as `key: value` pairs".to_owned(),
            ],
        ));
    }

    Ok(YamlMapping {
        document: Rc::new(document),
        node: Rc::new(root),
        prefix: String::new(),
    })
}

pub fn parse_yaml_mapping(text: &str, file: &str) -> Result<YamlMapping> {
    parse_checked(text, file, 0, || {
        config_error(
            format!("{file} is empty"),
            [
                "expected a YAML mapping",
                "add the keys this format requires",
            ],
        )
    })
}

pub fn parse_frontmatter_mapping(text: &str, file: &str) -> Result<YamlMapping> {
    let found = frontmatter::frontmatter(text, file)?;

    parse_checked(&found.block, file, found.line_offset, || {
        frontmatter::empty(file)
    })
}

fn read_source(path: &Path, file: &str) -> Result<String> {
    read_text(path).map_err(|error| {
        config_error(
            format!("cannot read {file}"),
            [
                io_message(&error, path),
                "check the path and its permissions".to_owned(),
            ],
        )
    })
}

pub fn read_yaml_mapping(path: &Path, file: &str) -> Result<YamlMapping> {
    parse_yaml_mapping(&read_source(path, file)?, file)
}

pub fn read_frontmatter_mapping(path: &Path, file: &str) -> Result<YamlMapping> {
    parse_frontmatter_mapping(&read_source(path, file)?, file)
}
