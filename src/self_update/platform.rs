//! Which released asset would replace the running ambit.
//!
//! Decided from the current process alone, with no network call, so every refusal self-update can
//! make (this machine has no binary, the directory is read-only) happens before anything is
//! downloaded.
//!
//! The asset names here are the third copy of one list: cargo-dist's `targets` produce them,
//! `install.sh` spells them from `uname`, and [`asset_name`] maps `std::env::consts::{OS, ARCH}`
//! onto them. All three have to agree, and a platform added to one is a 404 until it is added to
//! the other two.

use std::path::{Path, PathBuf};

/// The five archives a release publishes, keyed by `(OS, ARCH)`.
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

/// The release archive for this machine, or `None` where no release ships one.
///
/// `os` and `arch` are `std::env::consts::{OS, ARCH}`. The names are cargo-dist's:
/// `ambit-<target triple>.tar.xz`, or `.zip` on Windows.
pub fn asset_name(os: &str, arch: &str) -> Option<&'static str> {
    ASSETS
        .iter()
        .find(|((asset_os, asset_arch), _)| *asset_os == os && *asset_arch == arch)
        .map(|(_, asset)| *asset)
}

/// The file a self-update replaces: the executable this process is running, with every symlink
/// resolved.
///
/// Resolved because `ambit` on the `PATH` is often a link into wherever it was really installed.
/// Writing over the link would leave the real binary at the old version and break any other link
/// pointing at it.
///
/// Falls back to the unresolved path when the link cannot be read, so the caller reports a
/// permission problem about a path the user recognizes rather than failing here.
pub fn running_binary(exec_path: &Path) -> PathBuf {
    let Ok(resolved) = crate::util::fs::canonicalize(exec_path) else {
        return exec_path.to_path_buf();
    };

    // Windows canonicalizes to a verbatim `\\?\C:\…` path. The plain spelling names the same file
    // and is the one a user recognizes in a message.
    let text = resolved.to_string_lossy();

    match text.strip_prefix(r"\\?\") {
        Some(plain) if cfg!(windows) && !plain.starts_with(r"UNC\") => PathBuf::from(plain),
        _ => resolved,
    }
}

/// Whether the binary can be replaced in place.
///
/// The directory is what is tested, not the file: the swap is a rename into the directory, so a
/// writable file in a read-only directory still cannot be updated.
///
/// Tested by creating a file there, since the permission bits alone answer wrongly under ACLs, for
/// root, and on Windows.
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
