//! The list-section driver: one flat array of entries, each naming its own event.
//!
//! Some harnesses keep hooks as `"hooks": [{"trigger": "PreToolUse", ...}, ...]` rather than one
//! array per event. Identity works as it does for the array-section driver (`json_array.rs`): a
//! managed key is `<Event>@<digest>`, the digest taken over the entry ambit writes, so entries
//! added outside ambit have digests ambit never plans and survive every merge and prune.
//!
//! The event half of the key is read from the entry's [`LIST_EVENT_FIELD`]. The field is part of the
//! shape rather than a profile parameter: state records only a file's format and shape, and
//! `status` and `prune` build their driver from state, so the key must be derivable from the file
//! with nothing else to go on. The digest already covers the event, which makes that half
//! redundant for identity; it is kept so a key reads the same in every hooks file.

use indexmap::IndexSet;

use crate::errors::{AmbitError, ExitCode, Result, config_error};
use crate::model::documents::format::{ConfigEntry, DocumentDriver};
use crate::model::documents::json::{parse_json_document, serialize_json_document};
use crate::model::documents::json_array::{
    array_entry_key, entry_digest, split_entry_key, with_section,
};
use crate::util::json::{JsonObject, JsonValue};

/// The field in each entry that names its event.
pub const LIST_EVENT_FIELD: &str = "trigger";

/// The key one entry reads back as, or `None` for an entry with no string event.
///
/// An entry without one cannot be one ambit wrote, so it is no key ambit would compare against.
fn key_of(entry: &JsonValue) -> Option<String> {
    let event = entry.get(LIST_EVENT_FIELD)?.as_str()?;

    Some(array_entry_key(event, entry))
}

/// The section as an array; anything unusable reads as absent.
fn list_of<'a>(document: &'a JsonObject, section: &str) -> Option<&'a Vec<JsonValue>> {
    document.get(section).and_then(JsonValue::as_array)
}

fn keys_of(text: Option<&str>, section: &str, file: &str) -> Result<IndexSet<String>> {
    let document = parse_json_document(text, file)?;

    Ok(list_of(&document, section)
        .into_iter()
        .flatten()
        .filter_map(key_of)
        .collect())
}

/// The list-section driver, with the root keys it seeds on a merge.
#[derive(Clone, Debug, Default)]
pub struct ListSectionDriver {
    /// Root keys to seed, e.g. a `version` the harness requires beside the list. Written only
    /// where the document does not already have the key; a removal applies none of them.
    pub root_defaults: JsonObject,
}

/// Builds the driver. `None` seeds nothing.
pub fn list_section_driver(root_defaults: Option<&JsonObject>) -> ListSectionDriver {
    ListSectionDriver {
        root_defaults: root_defaults.cloned().unwrap_or_default(),
    }
}

impl DocumentDriver for ListSectionDriver {
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
            None => Vec::new(),
            Some(JsonValue::Array(existing)) => existing.clone(),
            Some(_) => {
                return Err(config_error(
                    format!("\"{section}\" in {file} is not a JSON array"),
                    [
                        format!("ambit appends one entry per managed hook to `{section}`"),
                        format!("make `{section}` an array, or move its current value aside"),
                    ],
                ));
            }
        };

        for entry in entries {
            let (event, digest) = split_entry_key(&entry.key, file)?;

            // An entry whose own event disagrees with its key would never be found again by
            // `section_keys`, so every install would append it once more.
            if entry
                .value
                .get(LIST_EVENT_FIELD)
                .and_then(JsonValue::as_str)
                != Some(event)
            {
                return Err(AmbitError::new(
                    ExitCode::Internal,
                    format!("cannot address \"{}\" in {file}", entry.key),
                    [
                        format!(
                            "an entry in a list section names its event in `{LIST_EVENT_FIELD}`, \
                             and this one does not name \"{event}\""
                        ),
                        "this is a bug in ambit".to_owned(),
                    ],
                ));
            }

            // Already there, by digest: skip it, so a second install is a no-op.
            if merged.iter().any(|item| entry_digest(item) == digest) {
                continue;
            }

            merged.push(entry.value.clone());
        }

        Ok(serialize_json_document(&with_section(
            &self.root_defaults,
            document,
            section,
            JsonValue::Array(merged),
        )))
    }

    /// The digest is the value, so presence of the key is the whole question.
    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        Ok(keys_of(text, section, file)?.contains(&entry.key))
    }

    /// The list survives emptying out, for the reason the array-section driver keeps an emptied
    /// event array: ambit owns entries in this file, not its containers.
    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>> {
        for key in keys {
            split_entry_key(key, file)?;
        }

        let mut document = parse_json_document(text, file)?;

        let Some(current) = list_of(&document, section) else {
            return Ok(None);
        };

        let wanted: IndexSet<&str> = keys.iter().map(String::as_str).collect();
        let remaining: Vec<JsonValue> = current
            .iter()
            .filter(|item| key_of(item).is_none_or(|key| !wanted.contains(key.as_str())))
            .cloned()
            .collect();

        // Nothing matched: no write to make, so a prune with nothing stale is byte-identical.
        if remaining.len() == current.len() {
            return Ok(None);
        }

        document.insert(section.to_owned(), JsonValue::Array(remaining));

        Ok(Some(serialize_json_document(&document)))
    }
}

#[cfg(test)]
mod tests;
