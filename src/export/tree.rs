//! The exported layout on disk, as a comparable tree.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::Result;
use crate::export::files::PackageFiles;
use crate::util::string_enum;

string_enum! {
    /// What a symlink in the export points at.
    pub enum LinkType {
        Dir => "dir",
        File => "file",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportEntry {
    /// `None` denotes a directory or a link.
    pub data: Option<Vec<u8>>,
    pub mode: u32,
    pub link: Option<String>,
    pub link_type: Option<LinkType>,
}

/// Export entries keyed by output-relative path.
pub type ExportTree = IndexMap<String, ExportEntry>;

/// One package as [`package_tree`] lays it out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreePackage {
    pub directory: String,
    pub files: PackageFiles,
}

/// Resolves existing ancestors without creating a missing output directory.
///
/// # Errors
///
/// Exit 2 when an existing ancestor cannot be resolved.
pub fn canonical_path(target: &Path) -> Result<PathBuf> {
    let _ = target;
    todo!("port export/tree.ts:canonicalPath")
}

/// Builds the exported layout, including relative links calculated for its final location.
pub fn package_tree(packages: &[TreePackage], output: &Path, link: bool) -> ExportTree {
    let _ = (packages, output, link);
    todo!("port export/tree.ts:packageTree")
}

/// Reads file contents and link targets without following symlinks in an existing export.
///
/// # Errors
///
/// Exit 2 when the export cannot be read.
pub fn read_tree(root: &Path) -> Result<ExportTree> {
    let _ = root;
    todo!("port export/tree.ts:readTree")
}

/// Compares JSON values, other file bytes, executable bits, and exact link targets.
pub fn same_entry(name: &str, expected: &ExportEntry, actual: &ExportEntry) -> bool {
    let _ = (name, expected, actual);
    todo!("port export/tree.ts:sameEntry")
}
