use indexmap::IndexSet;
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstInputValue, CstRootNode};

use crate::errors::{Result, config_error};
use crate::model::documents::format::{ConfigEntry, DocumentDriver};
use crate::util::json::{JsonObject, JsonValue, format_number, js_key_order, structurally_equal};

const EMPTY_TEXT: &str = "{}\n";

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

fn parse(text: &str, file: &str) -> Result<(CstRootNode, JsonObject)> {
    let root = CstRootNode::parse(text, &parse_options())
        .map_err(|error| syntax_error(file, error.range().start))?;

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

        // None only for a repeated section key: parsed value is the last, the tree finds the first.
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
