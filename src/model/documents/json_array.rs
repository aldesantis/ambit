//! The array-section driver: `.claude/settings.json`, `.cursor/hooks.json`, `.codex/hooks.json`,
//! `.gemini/settings.json`.
//!
//! Most harnesses write hooks as `<Event>: [entries]`. An array has no identity key: nothing in
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
//! Parsing and serialization are shared with the map-shaped JSON driver (`json.rs`): same files,
//! same syntax, differing only in what happens to one section of them.

use indexmap::{IndexMap, IndexSet};

use crate::errors::{AmbitError, ExitCode, Result, config_error};
use crate::model::documents::format::{ConfigEntry, DocumentDriver};
use crate::model::documents::json::{parse_json_document, section_of, serialize_json_document};
use crate::util::hash::sha256_hex;
use crate::util::json::{self, JsonObject, JsonValue};

/// How much of the SHA-256 a key carries: 48 bits.
///
/// A file holds only a handful of hooks, so collision risk is not worth widening a key that a
/// person reads in `.ambit/state.json` and in every `status` row.
pub const DIGEST_LENGTH: usize = 12;

/// What separates the event from the digest.
///
/// Neither half can contain it: an event is a `PascalCase` name from a closed set, a digest is hex.
/// So the key parses back unambiguously, which `remove_keys` needs to act from state alone.
const DIGEST_SEPARATOR: char = '@';

/// The digest of one entry as ambit would write it: `sha256(util::json::stringify(value))`,
/// truncated to [`DIGEST_LENGTH`].
///
/// Canonical JSON here means no whitespace, with keys kept in the order [`ConfigEntry`] already
/// promises (the order the profile's renderer built them in). Sorting keys would make the digest
/// describe something other than the bytes on disk, since parsing preserves original key order.
///
/// Nothing outside the entry feeds the digest: no timestamps, no absolute paths, no environment.
/// Two people installing the same bundle get the same digests, which the lock relies on.
pub fn entry_digest(value: &JsonValue) -> String {
    let mut digest = sha256_hex(json::stringify(value).as_bytes());
    digest.truncate(DIGEST_LENGTH);
    digest
}

/// The key within the section that names one entry: `<Event>@<digest>`.
pub fn array_entry_key(event: &str, value: &JsonValue) -> String {
    format!("{event}{DIGEST_SEPARATOR}{}", entry_digest(value))
}

/// Reads a key back into the event and digest it names.
///
/// # Errors
///
/// Exit 1 for a key this driver could not have produced. Both callers (append and remove) reach
/// the file through such a key, so guessing at it would mean writing a duplicate hook or leaving an
/// entry ambit claims to own in place forever.
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

/// Every entry in the section, keyed the way state records it.
///
/// An event whose value is not an array is skipped rather than refused: it is not a collision with
/// anything ambit would write, and an unusable section is `merge_section`'s error to raise, since
/// that is the code which cannot proceed with it.
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

/// The merged document: `defaults` the document lacks, then the document, with `section` set to
/// `value`.
///
/// Defaults come first, so a file ambit creates reads in the order a person would write it. The
/// document's own keys then keep their values and positions. Shared with the list driver
/// (`json_list.rs`), which seeds root keys the same way.
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

/// The array-section driver, with the root keys it seeds on a merge.
#[derive(Clone, Debug, Default)]
pub struct ArraySectionDriver {
    /// Root keys to seed, e.g. Cursor's `version: 1`. Written only where the document does not
    /// already have the key, so ambit adds it on file creation but never overwrites a `version: 2`
    /// someone else wrote. A removal applies none of them: pruning must not add keys.
    pub root_defaults: JsonObject,
}

/// Builds the driver. `None` seeds nothing.
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

            // Already there, by digest: skip it, so a second install is a no-op instead of adding a
            // duplicate hook.
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

    /// The digest is the value, so presence of the key is the whole question. An entry a person
    /// reformatted has a different digest and is therefore a different entry.
    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool> {
        Ok(keys_of(text, section, file)?.contains(&entry.key))
    }

    /// Both the section and an event's array survive emptying out. ambit owns entries inside this
    /// file, not its containers, and a person may add a hook of their own to the array it just
    /// vacated.
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

        // Nothing matched: no write to make. Keeps a prune with nothing stale byte-identical, and
        // does not recreate a file someone deleted by hand.
        if !removed {
            return Ok(None);
        }

        document.insert(section.to_owned(), JsonValue::Object(kept));

        Ok(Some(serialize_json_document(&document)))
    }
}

#[cfg(test)]
mod tests;
