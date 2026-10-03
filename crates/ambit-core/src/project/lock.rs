//! `ambit.lock`: the resolution result, written so an install can be reproduced.
//!
//! This is the half that builds and writes the document. The half that reads it (where the file
//! lives, and the pins catalog loading resolves against) is `model/lock_file.rs`; the shared
//! constants live there and are re-exported here so a caller finds all of "the lock" in one place.
//!
//! The `catalogs` section is an input; every other section is a record. `--frozen` compares the
//! lock as text, so a file that would be rewritten is out of date regardless of what the two
//! documents mean. The one exception is `commit` under `catalogs`: `read_catalog_pins` reads it
//! back and resolution goes to that commit, so it must be true, not just byte-equal.
//!
//! A pin is void once the config it was resolved from changes. Each entry records the `source` and
//! `ref` its commit came from, so a reader can tell a pin worth honoring from a stale one. Editing
//! `ref:` invalidates the pin as it always did.
//!
//! Byte-stability is the contract for everything written here: emit through [`emit_yaml`], and
//! hold nothing a second run could disagree about (no timestamps, no absolute paths, no cache
//! locations, a commit only where a source actually has one). Every value is machine-independent so
//! a committed lock stays shared across a team; a path into someone's local cache would make it
//! per-machine and produce a diff on every developer's first install.

use std::path::Path;

use indexmap::IndexMap;
use serde_json::json;

use crate::errors::{Result, drift_error};
use crate::model::catalog::Catalog;
use crate::model::hook_entity::HookType;
use crate::model::requirement::ItemKind;
use crate::model::yaml::emit_yaml;
use crate::resolution::resolve::{Bundle, BundleItem, format_reason, reason_of};
use crate::util::fs;
use crate::util::json::{JsonObject, JsonValue};
use crate::util::path::to_slash;

pub use crate::model::lock_file::{
    LOCK_FILENAME, LOCK_VERSION, lock_file_path, read_catalog_pins, read_lock_text,
};

/// One configured catalog, pinned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockCatalog {
    /// The `source` as config wrote it.
    pub source: String,
    /// The `ref` as config wrote it, absent when the entry named none.
    pub r#ref: Option<String>,
    /// The commit the ref resolved to. Absent for a `path:` source, which has no revision.
    pub commit: Option<String>,
}

/// One selected pack, explained.
///
/// No `path` and no `commit`: a pack materializes nothing and ships no bytes. It is recorded
/// because the reason line on every skill, server, and hook it pulled in names it, and those
/// reasons need something in the lock to resolve against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockPack {
    /// The catalog it came from.
    pub catalog: String,
    /// Why it is in the bundle, in `--explain`'s short form.
    pub reason: String,
}

/// One selected skill, pinned and explained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockSkill {
    /// The catalog it came from.
    pub catalog: String,
    /// Its directory within that source, `/`-separated.
    pub path: String,
    /// The commit those bytes came from, when the source has one.
    pub commit: Option<String>,
    /// Why it is in the bundle, in `--explain`'s short form.
    pub reason: String,
}

/// One selected MCP server, explained.
///
/// No `commit`, deliberately: a server is a handful of config values rather than a tree of files,
/// so the catalog entry's commit already says everything a reader could act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockMcp {
    /// The catalog it came from.
    pub catalog: String,
    /// Why it is in the bundle, in `--explain`'s short form.
    pub reason: String,
}

/// One selected hook, explained, and pinned when it ships bytes.
///
/// A hook is config values rendered into a harness file, or, when its `command` names a script the
/// hook's directory ships, also a tree of files to materialize. `path` and `commit` appear only in
/// the second case, the same reason [`LockSkill`] carries them and [`LockMcp`] does not. A hook
/// whose command is a command line takes [`LockMcp`]'s shape instead.
///
/// `path` is the hook's directory within its source, like [`LockSkill::path`]. It is never the
/// command ambit writes into a harness file, since that command is rewritten per harness and is not
/// one value the lock could hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockHook {
    /// The catalog it came from.
    pub catalog: String,
    /// Its directory within that source, `/`-separated. Present only when it ships a script.
    pub path: Option<String>,
    /// The commit those bytes came from, when the source has one.
    pub commit: Option<String>,
    /// Why it is in the bundle, in `--explain`'s short form.
    pub reason: String,
}

/// A lock document.
///
/// The five sections are keyed maps, not lists: a name is the identity of everything in them, and
/// a map makes a diff show one changed entry instead of a reordered list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lock {
    pub version: i64,
    /// Every configured catalog, not only those that contributed to the bundle.
    pub catalogs: IndexMap<String, LockCatalog>,
    pub packs: IndexMap<String, LockPack>,
    pub skills: IndexMap<String, LockSkill>,
    pub mcps: IndexMap<String, LockMcp>,
    pub hooks: IndexMap<String, LockHook>,
}

/// One bundle item's reason, in `--explain`'s short form.
fn reason(bundle: &Bundle, kind: ItemKind, name: &str) -> Result<String> {
    let item = BundleItem {
        kind,
        name: name.to_owned(),
    };

    Ok(format_reason(reason_of(bundle, &item)?))
}

/// Builds the lock for a resolved project.
///
/// Pure: what the lock says is a function of what resolution decided, so a test can compare two
/// locks without touching disk.
///
/// Every configured catalog is listed, even one that contributed nothing to this bundle. The lock
/// pins the inputs, and a catalog whose commit moves changes what a later resolve selects even
/// though today's bundle never named it. `catalogs` is in config order.
///
/// # Errors
///
/// Exit 1 if the bundle cannot account for one of its own items: a bug, not anything a catalog can
/// cause.
pub fn build_lock(catalogs: &[Catalog], bundle: &Bundle) -> Result<Lock> {
    let mut lock = Lock {
        version: LOCK_VERSION,
        catalogs: IndexMap::new(),
        packs: IndexMap::new(),
        skills: IndexMap::new(),
        mcps: IndexMap::new(),
        hooks: IndexMap::new(),
    };

    for catalog in catalogs {
        lock.catalogs.insert(
            catalog.name.clone(),
            LockCatalog {
                source: catalog.source.clone(),
                r#ref: catalog.r#ref.clone(),
                commit: catalog.commit.clone(),
            },
        );
    }

    for pack in &bundle.packs {
        lock.packs.insert(
            pack.name.clone(),
            LockPack {
                catalog: pack.catalog.clone(),
                reason: reason(bundle, ItemKind::Pack, &pack.name)?,
            },
        );
    }

    for skill in &bundle.skills {
        lock.skills.insert(
            skill.name.clone(),
            LockSkill {
                catalog: skill.catalog.clone(),
                path: skill.path.clone(),
                commit: skill.commit.clone(),
                reason: reason(bundle, ItemKind::Skill, &skill.name)?,
            },
        );
    }

    for mcp in &bundle.mcps {
        lock.mcps.insert(
            mcp.name.clone(),
            LockMcp {
                catalog: mcp.catalog.clone(),
                reason: reason(bundle, ItemKind::Mcp, &mcp.name)?,
            },
        );
    }

    for hook in &bundle.hooks {
        // A hook with no script has no bytes to pin, so it records neither path nor commit; see
        // `LockHook`.
        let ships = hook.r#type == HookType::Script;

        lock.hooks.insert(
            hook.name.clone(),
            LockHook {
                catalog: hook.catalog.clone(),
                path: ships.then(|| hook.path.clone()),
                commit: if ships { hook.commit.clone() } else { None },
                reason: reason(bundle, ItemKind::Hook, &hook.name)?,
            },
        );
    }

    Ok(lock)
}

/// One name-keyed section as a JSON object. Insertion order is irrelevant: emission sorts keys.
fn section<T>(entries: &IndexMap<String, T>, value: impl Fn(&T) -> JsonObject) -> JsonValue {
    JsonValue::Object(
        entries
            .iter()
            .map(|(name, entry)| (name.clone(), JsonValue::Object(value(entry))))
            .collect(),
    )
}

/// Inserts `key` only when there is a value for it, so an absent value leaves no key in the lock.
fn insert_some(object: &mut JsonObject, key: &str, value: Option<&String>) {
    if let Some(value) = value {
        object.insert(key.to_owned(), json!(value));
    }
}

/// The lock as the JSON document [`emit_yaml`] renders.
fn lock_document(lock: &Lock) -> JsonValue {
    json!({
        "version": lock.version,
        "catalogs": section(&lock.catalogs, |catalog| {
            let mut object = JsonObject::new();

            object.insert("source".to_owned(), json!(catalog.source));
            insert_some(&mut object, "ref", catalog.r#ref.as_ref());
            insert_some(&mut object, "commit", catalog.commit.as_ref());
            object
        }),
        "packs": section(&lock.packs, |pack| {
            let mut object = JsonObject::new();

            object.insert("catalog".to_owned(), json!(pack.catalog));
            object.insert("reason".to_owned(), json!(pack.reason));
            object
        }),
        "skills": section(&lock.skills, |skill| {
            let mut object = JsonObject::new();

            object.insert("catalog".to_owned(), json!(skill.catalog));
            object.insert("path".to_owned(), json!(skill.path));
            insert_some(&mut object, "commit", skill.commit.as_ref());
            object.insert("reason".to_owned(), json!(skill.reason));
            object
        }),
        "mcps": section(&lock.mcps, |mcp| {
            let mut object = JsonObject::new();

            object.insert("catalog".to_owned(), json!(mcp.catalog));
            object.insert("reason".to_owned(), json!(mcp.reason));
            object
        }),
        "hooks": section(&lock.hooks, |hook| {
            let mut object = JsonObject::new();

            object.insert("catalog".to_owned(), json!(hook.catalog));
            insert_some(&mut object, "path", hook.path.as_ref());
            insert_some(&mut object, "commit", hook.commit.as_ref());
            object.insert("reason".to_owned(), json!(hook.reason));
            object
        }),
    })
}

/// Renders a lock as the bytes written to disk.
///
/// Empty sections are emitted as empty maps, not omitted, so a project that loses its last MCP
/// server shows `mcps: {}` in the diff instead of a vanished key.
pub fn serialize_lock(lock: &Lock) -> String {
    emit_yaml(&lock_document(lock))
}

/// Writes a project's lock.
///
/// # Errors
///
/// Exit 1 when the file cannot be written, the catch-all code for an unanticipated failure.
pub fn write_lock_text(project_dir: &Path, text: &str) -> Result<()> {
    fs::write_text(&lock_file_path(project_dir), text)?;

    Ok(())
}

/// Asserts that the lock on disk is exactly what resolution would write: the check `--frozen` is.
///
/// Called before anything is materialized, so a CI run that fails this leaves the project
/// untouched. `expected` is the serialized lock resolution produced.
///
/// # Errors
///
/// Exit 5 when the project has no lock, or has one that differs.
pub fn assert_lock_current(project_dir: &Path, expected: &str) -> Result<()> {
    let actual = read_lock_text(project_dir)?;

    if actual.as_deref() == Some(expected) {
        return Ok(());
    }

    let reason = if actual.is_none() {
        format!(
            "`--frozen` compares against a committed lock, and {} has no {LOCK_FILENAME}",
            to_slash(project_dir)
        )
    } else {
        format!("resolving this project produces a different {LOCK_FILENAME} than the one on disk")
    };

    Err(drift_error(
        format!("{LOCK_FILENAME} is out of date"),
        [
            reason,
            "run `ambit install` without `--frozen`, then commit the result".to_owned(),
        ],
    ))
}

#[cfg(all(test, feature = "cli"))]
mod tests;
