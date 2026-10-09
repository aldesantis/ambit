use std::path::Path;

use indexmap::IndexMap;
use serde_json::json;

use crate::errors::{AmbitError, Result, config_error, drift_error};
use crate::model::catalog::Catalog;
use crate::model::hook_entity::HookType;
use crate::model::requirement::ItemKind;
use crate::model::yaml::emit_yaml;
use crate::project::exec::{hook_exec, mcp_exec};
use crate::resolution::resolve::{Bundle, BundleItem, format_reason, reason_of};
use crate::util::fs::{self, io_message};
use crate::util::hash::tree_digest;
use crate::util::json::{JsonObject, JsonValue};
use crate::util::path::{join, to_slash};

pub use crate::model::lock_file::{
    LOCK_FILENAME, LOCK_VERSION, LockedItems, lock_file_path, read_catalog_pins, read_lock_text,
    read_locked_items,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockCatalog {
    pub source: String,
    pub r#ref: Option<String>,
    pub commit: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockPack {
    pub catalog: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockSkill {
    pub catalog: String,
    pub path: String,
    pub commit: Option<String>,
    pub digest: Option<String>,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockMcp {
    pub catalog: String,
    pub exec: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockHook {
    pub catalog: String,
    pub path: Option<String>,
    pub commit: Option<String>,
    pub digest: Option<String>,
    pub exec: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lock {
    pub version: i64,
    pub catalogs: IndexMap<String, LockCatalog>,
    pub packs: IndexMap<String, LockPack>,
    pub skills: IndexMap<String, LockSkill>,
    pub mcps: IndexMap<String, LockMcp>,
    pub hooks: IndexMap<String, LockHook>,
}

fn reason(bundle: &Bundle, kind: ItemKind, name: &str) -> Result<String> {
    let item = BundleItem {
        kind,
        name: name.to_owned(),
    };

    Ok(format_reason(reason_of(bundle, &item)?))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemDigests {
    pub skills: IndexMap<String, String>,
    pub hooks: IndexMap<String, String>,
}

fn unhashable(kind: ItemKind, name: &str, dir: &Path, error: &std::io::Error) -> AmbitError {
    config_error(
        format!("cannot read the files of {kind} \"{name}\""),
        [
            io_message(error, dir),
            "make the catalog checkout readable, or delete it from the cache so the next run fetches it again".to_owned(),
        ],
    )
}

pub fn item_digests(bundle: &Bundle) -> Result<ItemDigests> {
    let mut digests = ItemDigests::default();

    for skill in bundle.skills.iter().filter(|skill| skill.commit.is_some()) {
        let dir = join(&skill.catalog_root, &skill.path);
        let digest = tree_digest(&dir)
            .map_err(|error| unhashable(ItemKind::Skill, &skill.name, &dir, &error))?;

        digests.skills.insert(skill.name.clone(), digest);
    }

    for hook in &bundle.hooks {
        if hook.commit.is_none() || hook.r#type != HookType::Script {
            continue;
        }

        let dir = join(&hook.catalog_root, &hook.path);
        let digest = tree_digest(&dir)
            .map_err(|error| unhashable(ItemKind::Hook, &hook.name, &dir, &error))?;

        digests.hooks.insert(hook.name.clone(), digest);
    }

    Ok(digests)
}

pub fn build_lock(catalogs: &[Catalog], bundle: &Bundle, digests: &ItemDigests) -> Result<Lock> {
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
                digest: skill
                    .commit
                    .as_ref()
                    .and_then(|_| digests.skills.get(&skill.name).cloned()),
                reason: reason(bundle, ItemKind::Skill, &skill.name)?,
            },
        );
    }

    for mcp in &bundle.mcps {
        lock.mcps.insert(
            mcp.name.clone(),
            LockMcp {
                catalog: mcp.catalog.clone(),
                exec: mcp_exec(&mcp.transport),
                reason: reason(bundle, ItemKind::Mcp, &mcp.name)?,
            },
        );
    }

    for hook in &bundle.hooks {
        let ships = hook.r#type == HookType::Script;
        let commit = if ships { hook.commit.clone() } else { None };
        let digest = commit
            .as_ref()
            .and_then(|_| digests.hooks.get(&hook.name).cloned());

        lock.hooks.insert(
            hook.name.clone(),
            LockHook {
                catalog: hook.catalog.clone(),
                path: ships.then(|| hook.path.clone()),
                exec: hook_exec(hook, digest.as_deref()),
                digest,
                commit,
                reason: reason(bundle, ItemKind::Hook, &hook.name)?,
            },
        );
    }

    Ok(lock)
}

fn section<T>(entries: &IndexMap<String, T>, value: impl Fn(&T) -> JsonObject) -> JsonValue {
    JsonValue::Object(
        entries
            .iter()
            .map(|(name, entry)| (name.clone(), JsonValue::Object(value(entry))))
            .collect(),
    )
}

fn insert_some(object: &mut JsonObject, key: &str, value: Option<&String>) {
    if let Some(value) = value {
        object.insert(key.to_owned(), json!(value));
    }
}

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
            insert_some(&mut object, "digest", skill.digest.as_ref());
            object.insert("reason".to_owned(), json!(skill.reason));
            object
        }),
        "mcps": section(&lock.mcps, |mcp| {
            let mut object = JsonObject::new();

            object.insert("catalog".to_owned(), json!(mcp.catalog));
            object.insert("exec".to_owned(), json!(mcp.exec));
            object.insert("reason".to_owned(), json!(mcp.reason));
            object
        }),
        "hooks": section(&lock.hooks, |hook| {
            let mut object = JsonObject::new();

            object.insert("catalog".to_owned(), json!(hook.catalog));
            insert_some(&mut object, "path", hook.path.as_ref());
            insert_some(&mut object, "commit", hook.commit.as_ref());
            insert_some(&mut object, "digest", hook.digest.as_ref());
            object.insert("exec".to_owned(), json!(hook.exec));
            object.insert("reason".to_owned(), json!(hook.reason));
            object
        }),
    })
}

pub fn serialize_lock(lock: &Lock) -> String {
    emit_yaml(&lock_document(lock))
}

pub fn write_lock_text(project_dir: &Path, text: &str) -> Result<()> {
    fs::write_text(&lock_file_path(project_dir), text)?;

    Ok(())
}

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

struct Pinned<'a> {
    kind: ItemKind,
    name: &'a str,
    path: Option<&'a String>,
    commit: Option<&'a String>,
    digest: Option<&'a String>,
}

fn digest_mismatch(entry: &Pinned<'_>, recorded: &str, actual: &str) -> AmbitError {
    let at = match (entry.commit, entry.path) {
        (Some(commit), Some(path)) => format!(" for {path} at commit {commit}"),
        _ => String::new(),
    };

    drift_error(
        format!(
            "{} \"{}\" does not match the digest {LOCK_FILENAME} records",
            entry.kind, entry.name
        ),
        [
            format!("{LOCK_FILENAME} records {recorded}{at}"),
            format!("the catalog checkout holds {actual}"),
            format!(
                "find out why these files changed while the commit did not; to accept them, delete this entry's `digest` from {LOCK_FILENAME} and run `ambit install` again"
            ),
        ],
    )
}

pub fn verify_digests(previous: &LockedItems, lock: &Lock) -> Result<()> {
    let skills = lock.skills.iter().map(|(name, skill)| {
        (
            Pinned {
                kind: ItemKind::Skill,
                name,
                path: Some(&skill.path),
                commit: skill.commit.as_ref(),
                digest: skill.digest.as_ref(),
            },
            previous.skills.get(name),
        )
    });
    let hooks = lock.hooks.iter().map(|(name, hook)| {
        (
            Pinned {
                kind: ItemKind::Hook,
                name,
                path: hook.path.as_ref(),
                commit: hook.commit.as_ref(),
                digest: hook.digest.as_ref(),
            },
            previous.hooks.get(name),
        )
    });

    for (entry, earlier) in skills.chain(hooks) {
        let (Some(earlier), Some(actual)) = (earlier, entry.digest) else {
            continue;
        };

        let Some(recorded) = &earlier.digest else {
            continue;
        };

        let same_pin = entry.commit.is_some()
            && entry.commit == earlier.commit.as_ref()
            && entry.path.is_some()
            && entry.path == earlier.path.as_ref();

        if same_pin && recorded != actual {
            return Err(digest_mismatch(&entry, recorded, actual));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
