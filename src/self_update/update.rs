//! `ambit self-update`: plan a release swap, then download, verify, extract, and swap it in.

use std::path::{Path, PathBuf};

use crate::errors::Result;
use crate::self_update::release::Http;

/// Everything self-update reads about the machine it runs on, gathered at the CLI boundary.
///
/// Passed in rather than read down here, so one command run sees one machine and a test can
/// describe a different one without touching the real environment.
#[derive(Clone)]
pub struct SelfContext<'a> {
    /// `std::env::consts::OS`.
    pub os: &'a str,
    /// `std::env::consts::ARCH`.
    pub arch: &'a str,
    /// The executable this process is running, before symlinks are resolved.
    pub exec_path: PathBuf,
    pub http: &'a dyn Http,
}

/// What an update would do, decided without downloading anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelfUpdatePlan {
    /// The version running now, as `Cargo.toml` spells it: no leading `v`.
    pub current: String,
    /// The release that would be installed, as a tag: with a leading `v`.
    pub target: String,
    /// The release archive for this machine.
    pub asset: String,
    /// The file that would be replaced, with symlinks resolved.
    pub binary: PathBuf,
    /// Whether `target` is a different release from `current`.
    pub changed: bool,
}

/// What `ambit self-update` would do, or the reason it cannot.
///
/// The local refusals are made in order of cost: whether this machine has an asset, then whether
/// the file can be written. Only after both does anything reach the network.
///
/// # Errors
///
/// Exit 2 when no release ships an asset for this platform, or when the binary's directory is not
/// writable; exit 4 when the latest release cannot be looked up.
pub fn plan_self_update(
    context: &SelfContext<'_>,
    requested: Option<&str>,
) -> Result<SelfUpdatePlan> {
    let _ = (context, requested);
    todo!("port self/update.ts:planSelfUpdate")
}

/// Puts `incoming` where `binary` is.
///
/// POSIX renames straight over the running executable: the running process keeps the old inode,
/// and the next run gets the new one. Windows refuses to replace a file that is open for execution,
/// but allows *renaming* it, so the old binary is moved aside first and the new one takes its name.
/// The displaced file cannot be deleted while it is still running, so a failure to remove it is
/// ignored; [`apply_self_update`] sweeps it up on the next run.
///
/// # Errors
///
/// The I/O error of a rename that failed.
pub fn swap_in_place(binary: &Path, incoming: &Path, windows: bool) -> std::io::Result<()> {
    let _ = (binary, incoming, windows);
    todo!("port self/update.ts:swapInPlace")
}

/// Downloads the planned release, verifies it, extracts the binary, and swaps it in.
///
/// # Errors
///
/// Exit 4 when the download fails, its hash does not match the release's `.sha256`, or the archive
/// does not contain the ambit binary. Either way the old binary is untouched and the download is
/// deleted.
pub fn apply_self_update(plan: &SelfUpdatePlan, context: &SelfContext<'_>) -> Result<()> {
    let _ = (plan, context);
    todo!("port self/update.ts:applySelfUpdate")
}

/// Whether the plan describes a move to a strictly newer release, as opposed to a downgrade.
pub fn is_upgrade(plan: &SelfUpdatePlan) -> bool {
    let _ = plan;
    todo!("port self/update.ts:isUpgrade")
}
