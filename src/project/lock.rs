//! Building, writing, and checking `ambit.lock`.

use std::path::Path;

use indexmap::IndexMap;

use crate::errors::Result;
use crate::model::catalog::Catalog;
use crate::resolution::resolve::Bundle;

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
/// because the reason line on every item it pulled in names it.
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
/// No `commit`, deliberately: a server is a handful of config values rather than a tree of files.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockMcp {
    /// The catalog it came from.
    pub catalog: String,
    /// Why it is in the bundle, in `--explain`'s short form.
    pub reason: String,
}

/// One selected hook, explained, and pinned when it ships bytes.
///
/// `path` and `commit` appear only when the hook's `command` names a script its directory ships;
/// a hook whose command is a command line takes [`LockMcp`]'s shape instead.
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

/// Builds the lock for a resolved project.
///
/// Pure. Every configured catalog is listed, even one that contributed nothing to this bundle: the
/// lock pins the inputs. `catalogs` is in config order.
///
/// # Errors
///
/// Exit 1 if the bundle cannot account for one of its own items: a bug, not anything a catalog can
/// cause.
pub fn build_lock(catalogs: &[Catalog], bundle: &Bundle) -> Result<Lock> {
    let _ = (catalogs, bundle);
    todo!("port project/lock.ts:buildLock")
}

/// Renders a lock as the bytes written to disk.
///
/// Empty sections are emitted as empty maps, not omitted, so a project that loses its last MCP
/// server shows `mcps: {}` in the diff instead of a vanished key.
pub fn serialize_lock(lock: &Lock) -> String {
    let _ = lock;
    todo!("port project/lock.ts:serializeLock")
}

/// Writes a project's lock.
///
/// # Errors
///
/// Exit 2 when the file cannot be written.
pub fn write_lock_text(project_dir: &Path, text: &str) -> Result<()> {
    let _ = (project_dir, text);
    todo!("port project/lock.ts:writeLockText")
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
    let _ = (project_dir, expected);
    todo!("port project/lock.ts:assertLockCurrent")
}
