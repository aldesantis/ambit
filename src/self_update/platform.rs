//! Asset names must match cargo-dist's `targets` and `install.sh`; a platform added to one
//! 404s until it is added to the others.

use std::path::{Path, PathBuf};

const ASSETS: &[((&str, &str), &str)] = &[
    (("macos", "aarch64"), "ambit-aarch64-apple-darwin.tar.xz"),
    (("macos", "x86_64"), "ambit-x86_64-apple-darwin.tar.xz"),
    (("linux", "x86_64"), "ambit-x86_64-unknown-linux-gnu.tar.xz"),
    (
        ("linux", "aarch64"),
        "ambit-aarch64-unknown-linux-gnu.tar.xz",
    ),
    (("windows", "x86_64"), "ambit-x86_64-pc-windows-msvc.zip"),
];

pub fn asset_name(os: &str, arch: &str) -> Option<&'static str> {
    ASSETS
        .iter()
        .find(|((asset_os, asset_arch), _)| *asset_os == os && *asset_arch == arch)
        .map(|(_, asset)| *asset)
}

/// Symlinks are resolved: writing over a link would leave the real binary at the old version.
pub fn running_binary(exec_path: &Path) -> PathBuf {
    crate::util::fs::canonicalize(exec_path).unwrap_or_else(|_| exec_path.to_path_buf())
}

/// Tests the directory (the swap is a rename into it) by creating a file, since permission
/// bits answer wrongly under ACLs, for root, and on Windows.
pub fn can_replace(binary: &Path) -> bool {
    let Some(directory) = binary.parent() else {
        return false;
    };
    let directory = if directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        directory
    };

    tempfile::Builder::new()
        .prefix(".ambit-write-test-")
        .tempfile_in(directory)
        .is_ok()
}

#[cfg(test)]
mod tests;
