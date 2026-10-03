//! The in-memory file set one exported package is built as.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageFile {
    /// `None` denotes a directory, including an empty asset directory.
    pub data: Option<Vec<u8>>,
    pub mode: u32,
    /// The file it was copied from, when it came from the catalog.
    pub source: Option<PathBuf>,
}

/// Package files keyed by package-relative path, in insertion order.
pub type PackageFiles = IndexMap<String, PackageFile>;

/// Adds one package file, refusing collisions instead of overwriting another component. The TS
/// default `mode` was `0o644`.
///
/// # Errors
///
/// Exit 2 when `target` is already in the package.
pub fn add_file(files: &mut PackageFiles, target: &str, data: Vec<u8>, mode: u32) -> Result<()> {
    let _ = (files, target, data, mode);
    todo!("port export/files.ts:addFile")
}

/// Dereferences assets inside the catalog, rejecting cycles and external symlink targets.
///
/// # Errors
///
/// Exit 2 for a symlink cycle, a symlink leaving the catalog, or a collision.
pub fn collect_files(
    files: &mut PackageFiles,
    source: &Path,
    destination: &str,
    catalog_root: &Path,
    exclude: &[String],
) -> Result<()> {
    let _ = (files, source, destination, catalog_root, exclude);
    todo!("port export/files.ts:collectFiles")
}
