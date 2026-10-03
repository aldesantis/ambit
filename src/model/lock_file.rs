//! Reading `ambit.lock`. Building and writing one lives in `project/lock.rs`.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::Result;
use crate::model::config::ProjectConfig;

/// The lockfile's name, at the project root beside `ambit.yml`.
pub const LOCK_FILENAME: &str = "ambit.lock";

/// The only lock version this build reads or writes.
pub const LOCK_VERSION: i64 = 1;

/// Where the lock lives for a project.
pub fn lock_file_path(project_dir: &Path) -> PathBuf {
    let _ = project_dir;
    todo!("port model/lock-file.ts:lockFilePath")
}

/// Reads a project's lock as text, returning `None` when there is none.
///
/// Text, because that is what `--frozen` compares: a lock that would be rewritten is out of date,
/// whatever the two documents mean.
///
/// # Errors
///
/// Exit 2 for a lock that exists but cannot be read; "there is no lock" and "your lock is
/// unreadable" call for different fixes.
pub fn read_lock_text(project_dir: &Path) -> Result<Option<String>> {
    let _ = project_dir;
    todo!("port model/lock-file.ts:readLockText")
}

/// The commit each configured catalog is pinned to, keyed by catalog name.
///
/// Empty for a project with no lock, since a project with nothing to reproduce should resolve
/// against its remote rather than inherit a shared clone's idea of `main`.
///
/// An entry survives only when the lock's `source` and `ref` still name the same repository and
/// revision `ambit.yml` does, and only when it has a commit at all. Three cases drop it: the config
/// moved (`ref:` edited, or `source:` repointed); the catalog is new since the lock was written; or
/// the source is `path:`, which has no revision to pin.
///
/// # Errors
///
/// Exit 2 for a lock that exists and cannot be read.
pub fn read_catalog_pins(
    project_dir: &Path,
    config: &ProjectConfig,
) -> Result<IndexMap<String, String>> {
    let _ = (project_dir, config);
    todo!("port model/lock-file.ts:readCatalogPins")
}
