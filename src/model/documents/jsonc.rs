//! The JSONC driver: `.opencode/opencode.jsonc`.
//!
//! JSONC is JSON with comments and trailing commas, which makes a parse round-trip lossy: a
//! person's comments would not survive it. So this driver never re-serializes the document. It
//! edits the `jsonc_parser` crate's concrete syntax tree, one key at a time, and renders the tree back, leaving
//! comments, blank lines, indentation and key order everywhere else untouched. Reads go through the
//! crate's serde conversion, so a reformatted entry is not drift.

use indexmap::IndexSet;
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstInputValue, CstRootNode};

use crate::errors::{Result, config_error};
use crate::model::documents::format::{ConfigEntry, DocumentDriver};
use crate::util::json::{JsonObject, JsonValue, format_number, js_key_order, structurally_equal};

/// The document a file that does not exist yet stands in as.
///
/// An empty object rather than an empty string: an edit needs somewhere to put a key, and starting
/// from `{}` makes a first install produce an ordinary document instead of a fragment.
const EMPTY_TEXT: &str = "{}\n";

/// Comments are always tolerated; trailing commas are legal JSONC and appear in real configs.
/// Everything else the crate can be lenient about (loose property names, single quotes, missing
/// commas, JSON5 numbers and escapes) is refused, as VS Code's JSONC parser refuses it.
fn parse_options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

fn syntax_error(file: &str, offset: usize) -> crate::errors::AmbitError {
    config_error(
        format!("{file} is not valid JSONC"),
        [
            format!("parse error at offset {offset}"),
            "correct the syntax, so ambit can add its own keys without discarding the rest"
                .to_owned(),
        ],
    )
}

/// Parses the whole document into its tree, refusing anything it cannot edit.
///
/// The offset in a syntax error is a byte offset into the text.
///
/// # Errors
///
/// Exit 2 for a document that cannot be parsed even tolerantly, or one whose root is not an
/// object.
fn parse(text: &str, file: &str) -> Result<(CstRootNode, JsonObject)> {
    let root = CstRootNode::parse(text, &parse_options())
        .map_err(|error| syntax_error(file, error.range().start))?;

    // A document with no value at all (empty, or only comments) is a syntax error, as it is to
    // VS Code's parser, reported where the value was expected: the end of the text.
    if root.value().is_none() {
        return Err(syntax_error(file, text.len()));
    }

    match root.to_serde_value() {
        Some(JsonValue::Object(document)) => Ok((root, document)),
        _ => Err(config_error(
            format!("{file} is not a JSONC object"),
            [
                "ambit merges its keys into this document, which requires an object at the root",
                "make the document an object, or move the file aside",
            ],
        )),
    }
}

fn section_of<'a>(document: &'a JsonObject, section: &str) -> Option<&'a JsonObject> {
    document.get(section).and_then(JsonValue::as_object)
}

/// A value in the shape the tree takes for an insert or a replacement.
///
/// Keys go in the order JavaScript enumerates them, so an entry renders in the same order the
/// JSON drivers would write it.
fn to_cst(value: &JsonValue) -> CstInputValue {
    match value {
        JsonValue::Null => CstInputValue::Null,
        JsonValue::Bool(b) => CstInputValue::Bool(*b),
        JsonValue::Number(n) => CstInputValue::Number(format_number(n)),
        JsonValue::String(s) => CstInputValue::String(s.clone()),
        JsonValue::Array(items) => CstInputValue::Array(items.iter().map(to_cst).collect()),
        JsonValue::Object(object) => CstInputValue::Object(
            js_key_order(object)
                .into_iter()
                .map(|key| (key.clone(), to_cst(&object[key.as_str()])))
                .collect(),
        ),
    }
}

/// The JSONC driver.
#[derive(Clone, Copy, Debug, Default)]
pub struct JsoncDriver;

impl DocumentDriver for JsoncDriver {
    fn section_keys(
        &self,
        text: Option<&str>,
        section: &str,
        file: &str,
    ) -> Result<IndexSet<String>> {
        let Some(text) = text else {
            return Ok(IndexSet::new());
        };

        let (_, document) = parse(text, file)?;

        Ok(section_of(&document, section)
            .map(|existing| js_key_order(existing).into_iter().cloned().collect())
            .unwrap_or_default())
    }

    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String> {
        let (root, document) = parse(text.unwrap_or(EMPTY_TEXT), file)?;

        if document
            .get(section)
            .is_some_and(|existing| !existing.is_object())
        {
            return Err(config_error(
                format!("\"{section}\" in {file} is not a JSONC object"),
                [
                    format!("ambit writes one key per managed entry inside `{section}`"),
                    format!("make `{section}` an object, or move its current value aside"),
                ],
            ));
        }

        let target = root.object_value_or_set().object_value_or_set(section);

        // An existing key is replaced where it stands; a new one is appended.
        for entry in entries {
            match target.get(&entry.key) {
                Some(prop) => prop.set_value(to_cst(&entry.value)),
                None => {
                    target.append(&entry.key, to_cst(&entry.value));
                }
            }
        }

        Ok(root.to_string())
    }

    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        let Some(text) = text else {
            return Ok(false);
        };

        let (_, document) = parse(text, file)?;

        Ok(section_of(&document, section)
            .and_then(|existing| existing.get(&entry.key))
            .is_some_and(|actual| structurally_equal(&entry.value, actual)))
    }

    /// The section itself is left in place even when it empties out, same as the JSON driver
    /// leaving `{}` behind.
    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>> {
        let Some(text) = text else {
            return Ok(None);
        };

        let (root, document) = parse(text, file)?;

        let Some(existing) = section_of(&document, section) else {
            return Ok(None);
        };

        if !keys.iter().any(|key| existing.contains_key(key)) {
            return Ok(None);
        }

        // Absent only for a document that repeats the section's key, where the parsed value is the
        // last occurrence and the tree finds the first.
        let Some(target) = root
            .object_value()
            .and_then(|object| object.object_value(section))
        else {
            return Ok(None);
        };

        for key in keys {
            if let Some(prop) = target.get(key) {
                prop.remove();
            }
        }

        Ok(Some(root.to_string()))
    }
}

#[cfg(test)]
mod tests;
