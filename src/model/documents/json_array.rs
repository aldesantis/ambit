use indexmap::{IndexMap, IndexSet};

use crate::errors::{AmbitError, ExitCode, Result, config_error};
use crate::model::documents::format::{ConfigEntry, DocumentDriver};
use crate::model::documents::json::{parse_json_document, section_of, serialize_json_document};
use crate::util::hash::sha256_hex;
use crate::util::json::{self, JsonObject, JsonValue};

pub const DIGEST_LENGTH: usize = 12;

const DIGEST_SEPARATOR: char = '@';

// Do not sort keys: parsing preserves key order, so the digest must hash the bytes as written.
pub fn entry_digest(value: &JsonValue) -> String {
    let mut digest = sha256_hex(json::stringify(value).as_bytes());
    digest.truncate(DIGEST_LENGTH);
    digest
}

pub fn array_entry_key(event: &str, value: &JsonValue) -> String {
    format!("{event}{DIGEST_SEPARATOR}{}", entry_digest(value))
}

pub(super) fn split_entry_key<'a>(key: &'a str, file: &str) -> Result<(&'a str, &'a str)> {
    match key.rfind(DIGEST_SEPARATOR) {
        Some(at) if at > 0 && at < key.len() - 1 => Ok((&key[..at], &key[at + 1..])),
        _ => Err(AmbitError::new(
            ExitCode::Internal,
            format!("cannot address \"{key}\" in {file}"),
            [
                format!(
                    "an entry in an array section is keyed `<Event>{DIGEST_SEPARATOR}<digest>`, \
                     and this is not"
                ),
                "this is a bug in ambit; deleting `.ambit/state.json` and installing again clears it"
                    .to_owned(),
            ],
        )),
    }
}

fn keys_of(text: Option<&str>, section: &str, file: &str) -> Result<IndexSet<String>> {
    let document = parse_json_document(text, file)?;
    let mut keys = IndexSet::new();

    for (event, entries) in section_of(&document, section).into_iter().flatten() {
        let Some(entries) = entries.as_array() else {
            continue;
        };

        for entry in entries {
            keys.insert(array_entry_key(event, entry));
        }
    }

    Ok(keys)
}

pub(super) fn with_section(
    defaults: &JsonObject,
    document: JsonObject,
    section: &str,
    value: JsonValue,
) -> JsonObject {
    let mut out = JsonObject::new();

    for (key, default) in defaults {
        if !document.contains_key(key) {
            out.insert(key.clone(), default.clone());
        }
    }

    out.extend(document);
    out.insert(section.to_owned(), value);
    out
}

#[derive(Clone, Debug, Default)]
pub struct ArraySectionDriver {
    pub root_defaults: JsonObject,
}

pub fn array_section_driver(root_defaults: Option<&JsonObject>) -> ArraySectionDriver {
    ArraySectionDriver {
        root_defaults: root_defaults.cloned().unwrap_or_default(),
    }
}

impl DocumentDriver for ArraySectionDriver {
    fn section_keys(
        &self,
        text: Option<&str>,
        section: &str,
        file: &str,
    ) -> Result<IndexSet<String>> {
        keys_of(text, section, file)
    }

    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String> {
        let document = parse_json_document(text, file)?;

        let mut merged = match document.get(section) {
            None => JsonObject::new(),
            Some(JsonValue::Object(existing)) => existing.clone(),
            Some(_) => {
                return Err(config_error(
                    format!("\"{section}\" in {file} is not a JSON object"),
                    [
                        format!("ambit appends its entries to the arrays inside `{section}`"),
                        format!("make `{section}` an object, or move its current value aside"),
                    ],
                ));
            }
        };

        for entry in entries {
            let (event, digest) = split_entry_key(&entry.key, file)?;

            let mut present = match merged.get(event) {
                None => Vec::new(),
                Some(JsonValue::Array(current)) => current.clone(),
                Some(_) => {
                    return Err(config_error(
                        format!("\"{section}.{event}\" in {file} is not a JSON array"),
                        [
                            format!(
                                "ambit appends one entry per managed hook to `{section}.{event}`"
                            ),
                            format!(
                                "make `{section}.{event}` an array, or move its current value aside"
                            ),
                        ],
                    ));
                }
            };

            if present.iter().any(|item| entry_digest(item) == digest) {
                continue;
            }

            present.push(entry.value.clone());
            merged.insert(event.to_owned(), JsonValue::Array(present));
        }

        Ok(serialize_json_document(&with_section(
            &self.root_defaults,
            document,
            section,
            JsonValue::Object(merged),
        )))
    }

    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        Ok(keys_of(text, section, file)?.contains(&entry.key))
    }

    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>> {
        let mut document = parse_json_document(text, file)?;

        let Some(existing) = section_of(&document, section) else {
            return Ok(None);
        };

        let mut wanted: IndexMap<&str, IndexSet<&str>> = IndexMap::new();

        for key in keys {
            let (event, digest) = split_entry_key(key, file)?;

            wanted.entry(event).or_default().insert(digest);
        }

        let mut kept = existing.clone();
        let mut removed = false;

        for (event, digests) in wanted {
            let Some(JsonValue::Array(current)) = kept.get(event) else {
                continue;
            };

            let remaining: Vec<JsonValue> = current
                .iter()
                .filter(|item| !digests.contains(entry_digest(item).as_str()))
                .cloned()
                .collect();

            if remaining.len() == current.len() {
                continue;
            }

            kept.insert(event.to_owned(), JsonValue::Array(remaining));
            removed = true;
        }

        if !removed {
            return Ok(None);
        }

        document.insert(section.to_owned(), JsonValue::Object(kept));

        Ok(Some(serialize_json_document(&document)))
    }
}

#[cfg(test)]
mod tests;
