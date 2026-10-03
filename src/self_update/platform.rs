//! Which release asset fits this machine, and whether the running binary can be replaced.

use std::path::{Path, PathBuf};

/// The release archive for this machine, or `None` where no release ships one.
///
/// `os` and `arch` are `std::env::consts::{OS, ARCH}`. The names are cargo-dist's:
/// `ambit-<target triple>.tar.xz`, or `.zip` on Windows.
pub fn asset_name(os: &str, arch: &str) -> Option<&'static str> {
    let _ = (os, arch);
    todo!("port self/platform.ts:assetName (cargo-dist asset names)")
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
    let _ = exec_path;
    todo!("port self/platform.ts:runningBinary")
}

/// Whether the binary can be replaced in place.
///
/// The directory is what is tested, not the file: the swap is a rename into the directory, so a
/// writable file in a read-only directory still cannot be updated.
pub fn can_replace(binary: &Path) -> bool {
    let _ = binary;
    todo!("port self/platform.ts:canReplace")
}
