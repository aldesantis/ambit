//! Replacing the running ambit binary with a released one.
//!
//! Split into a plan and an apply, because everything that can refuse the update is knowable before
//! a byte is downloaded: whether a release ships an asset for this machine, whether the file can be
//! written at all. `--dry-run` is the plan on its own.
//!
//! Two invariants the apply holds:
//!
//! - **The bytes are verified before they are installed.** The archive is hashed as it streams and
//!   checked against the release's `<asset>.sha256`, the same file `install.sh` checks against. A
//!   mismatch deletes the download and leaves the old binary in place. There is no flag to skip it.
//! - **The swap is a rename.** A rename within one directory is atomic, so an interrupted update
//!   leaves either the old binary or the new one and never a half-written file where ambit used to
//!   be. Windows cannot rename over a running executable, so there the old one is moved aside
//!   first; see [`swap_in_place`].
//!
//! The archive layout is cargo-dist's: a `.tar.xz` holds `ambit-<triple>/ambit`, a `.zip` holds
//! `ambit.exe`. Either is accepted at the archive root or under the one `ambit-<triple>/`
//! directory, so a change in how dist nests the zip does not break every Windows update.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::errors::{AmbitError, Result, config_error, network_error};
use crate::self_update::platform::{asset_name, can_replace, running_binary};
use crate::self_update::release::{
    CHECKSUM_SUFFIX, DOWNLOAD_TIMEOUT, Http, METADATA_TIMEOUT, as_tag, checksum_for,
    download_asset, fetch_asset_text, is_newer, latest_tag,
};
use crate::util::fs::rm_rf;
use crate::version::VERSION;

/// Suffix of the extracted binary while it waits to be swapped in, beside the binary it replaces.
const INCOMING_SUFFIX: &str = ".incoming";

/// Suffix of the downloaded archive while it is still unverified, beside the binary it may replace.
const DOWNLOAD_SUFFIX: &str = ".download";

/// Suffix of the displaced binary on Windows, which cannot rename over a running executable.
const DISPLACED_SUFFIX: &str = ".old";

/// The archive extensions cargo-dist uses, `.tar.xz` everywhere but Windows.
const TAR_XZ: &str = ".tar.xz";
const ZIP: &str = ".zip";

/// Everything self-update reads about the machine it runs on, gathered at the CLI boundary.
///
/// Passed in rather than read down here, so one command run sees one machine and a test can
/// describe a different one without touching the real environment. Same reason
/// [`source_context_of`](crate::cli::commands::source_context_of) exists for the commands that
/// resolve catalogs.
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

/// `path` with `suffix` appended to its file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text: OsString = path.as_os_str().to_owned();
    text.push(suffix);
    PathBuf::from(text)
}

/// What `ambit self-update` would do, or the reason it cannot.
///
/// The local refusals are made in order of cost: whether this machine has an asset, then whether
/// the file can be written. Only after both does anything reach the network, so a user on a
/// read-only install is told so immediately rather than after a download.
///
/// # Errors
///
/// Exit 2 when no release ships an asset for this platform, or when the binary's directory is not
/// writable; exit 4 when the latest release cannot be looked up.
pub fn plan_self_update(
    context: &SelfContext<'_>,
    requested: Option<&str>,
) -> Result<SelfUpdatePlan> {
    let Some(asset) = asset_name(context.os, context.arch) else {
        return Err(config_error(
            format!(
                "no ambit binary is published for {}-{}",
                context.os, context.arch
            ),
            [
                "this build cannot replace itself with one that does not exist",
                "build it from source: `cargo install --locked --git https://github.com/nebulab/ambit`",
            ],
        ));
    };

    let binary = running_binary(&context.exec_path);

    if !can_replace(&binary) {
        return Err(config_error(
            format!("cannot write to the directory holding {}", binary.display()),
            [
                "self-update replaces the binary in place, and that needs write access to its directory",
                "reinstall it somewhere writable, or run the install script with the permissions it needs:",
                "curl -fsSL https://raw.githubusercontent.com/nebulab/ambit/main/install.sh | sh",
            ],
        ));
    }

    let target = match requested {
        None => latest_tag(context.http, METADATA_TIMEOUT)?,
        Some(requested) => as_tag(requested),
    };

    Ok(SelfUpdatePlan {
        current: VERSION.to_owned(),
        changed: as_tag(VERSION) != target,
        target,
        asset: asset.to_owned(),
        binary,
    })
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
    if !windows {
        return std::fs::rename(incoming, binary);
    }

    let displaced = with_suffix(binary, DISPLACED_SUFFIX);

    std::fs::rename(binary, &displaced)?;

    if let Err(error) = std::fs::rename(incoming, binary) {
        std::fs::rename(&displaced, binary)?;
        return Err(error);
    }

    // Still running, on Windows. The next self-update removes it.
    let _ = rm_rf(&displaced);

    Ok(())
}

/// The error for an archive that holds no ambit binary, or one that cannot be read as an archive.
fn missing_binary(asset: &str, reason: impl Into<String>) -> AmbitError {
    network_error(
        format!("{asset} does not contain the ambit binary"),
        [
            reason.into(),
            "report it at https://github.com/nebulab/ambit/issues".to_owned(),
        ],
    )
}

/// Whether an archive member is the binary: `<binary_name>` at the root, or under the one
/// `ambit-<triple>/` directory cargo-dist nests archives in.
fn is_binary_member(member: &str, root_dir: &str, binary_name: &str) -> bool {
    let member = member.strip_prefix("./").unwrap_or(member);

    member == binary_name || member.strip_prefix(root_dir) == Some(binary_name)
}

/// Writes `reader` to `incoming`, executable where the platform has the bit.
fn write_incoming(reader: &mut dyn Read, incoming: &Path) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        options.mode(0o755);
    }

    let mut file = options.open(incoming)?;
    std::io::copy(reader, &mut file)?;

    Ok(())
}

/// Extracts the ambit binary out of a verified archive into `incoming`.
///
/// # Errors
///
/// Exit 4 when the archive cannot be read, or holds no ambit binary.
fn extract_binary(archive: &Path, asset: &str, windows: bool, incoming: &Path) -> Result<()> {
    let binary_name = if windows { "ambit.exe" } else { "ambit" };
    let stem = asset
        .strip_suffix(TAR_XZ)
        .or_else(|| asset.strip_suffix(ZIP))
        .unwrap_or(asset);
    let root_dir = format!("{stem}/");
    let unreadable = |error: &dyn std::fmt::Display| {
        missing_binary(asset, format!("the archive could not be read: {error}"))
    };
    let file = std::fs::File::open(archive).map_err(|error| unreadable(&error))?;

    if asset.ends_with(ZIP) {
        let mut zip = zip::ZipArchive::new(file).map_err(|error| unreadable(&error))?;
        let name = zip
            .file_names()
            .find(|member| is_binary_member(member, &root_dir, binary_name))
            .map(str::to_owned);
        let Some(name) = name else {
            return Err(missing_binary(asset, format!("no {binary_name} inside it")));
        };
        let mut member = zip.by_name(&name).map_err(|error| unreadable(&error))?;

        return write_incoming(&mut member, incoming).map_err(|error| unreadable(&error));
    }

    let xz = lzma_rust2::XzReader::new(std::io::BufReader::new(file), true);
    let mut tar = tar::Archive::new(xz);

    for entry in tar.entries().map_err(|error| unreadable(&error))? {
        let mut entry = entry.map_err(|error| unreadable(&error))?;
        let path = entry
            .path()
            .map_err(|error| unreadable(&error))?
            .to_string_lossy()
            .replace('\\', "/");

        if entry.header().entry_type().is_file() && is_binary_member(&path, &root_dir, binary_name)
        {
            return write_incoming(&mut entry, incoming).map_err(|error| unreadable(&error));
        }
    }

    Err(missing_binary(
        asset,
        format!("no {root_dir}{binary_name} inside it"),
    ))
}

/// Downloads the planned release, verifies it, extracts the binary, and swaps it in.
///
/// # Errors
///
/// Exit 4 when the download fails, its hash does not match the release's `.sha256`, or the archive
/// does not contain the ambit binary. Either way the old binary is untouched and the download is
/// deleted.
pub fn apply_self_update(plan: &SelfUpdatePlan, context: &SelfContext<'_>) -> Result<()> {
    let windows = context.os == "windows";
    let incoming = with_suffix(&plan.binary, INCOMING_SUFFIX);
    let download = with_suffix(&plan.binary, DOWNLOAD_SUFFIX);

    // A leftover from a Windows update that could not delete its own displaced binary while it was
    // still running. Harmless, but it is this command's mess to clear.
    if windows {
        rm_rf(&with_suffix(&plan.binary, DISPLACED_SUFFIX))?;
    }

    let checksums = fetch_asset_text(
        context.http,
        &plan.target,
        &format!("{}{CHECKSUM_SUFFIX}", plan.asset),
        METADATA_TIMEOUT,
    )?;
    let expected = checksum_for(&checksums, &plan.asset)?;

    let applied = (|| -> Result<()> {
        let actual = download_asset(
            context.http,
            &plan.target,
            &plan.asset,
            &download,
            DOWNLOAD_TIMEOUT,
        )?;

        if actual != expected {
            return Err(network_error(
                format!("checksum mismatch for {}", plan.asset),
                [
                    format!("expected {expected}, got {actual}"),
                    "the download was discarded and the installed ambit was left alone".to_owned(),
                    "try again; if it keeps happening, report it at https://github.com/nebulab/ambit/issues"
                        .to_owned(),
                ],
            ));
        }

        extract_binary(&download, &plan.asset, windows, &incoming)?;
        swap_in_place(&plan.binary, &incoming, windows)?;

        Ok(())
    })();

    let cleaned = rm_rf(&incoming).and_then(|()| rm_rf(&download));

    applied?;
    cleaned?;

    Ok(())
}

/// Whether the plan describes a move to a strictly newer release, as opposed to a downgrade.
pub fn is_upgrade(plan: &SelfUpdatePlan) -> bool {
    is_newer(&plan.current, &plan.target)
}

#[cfg(test)]
mod tests;
