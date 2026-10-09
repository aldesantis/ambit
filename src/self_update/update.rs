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

const INCOMING_SUFFIX: &str = ".incoming";

const DOWNLOAD_SUFFIX: &str = ".download";

const DISPLACED_SUFFIX: &str = ".old";

const TAR_XZ: &str = ".tar.xz";
const ZIP: &str = ".zip";

#[derive(Clone)]
pub struct SelfContext<'a> {
    pub os: &'a str,
    pub arch: &'a str,
    pub exec_path: PathBuf,
    pub http: &'a dyn Http,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelfUpdatePlan {
    pub current: String,
    pub target: String,
    pub asset: String,
    pub binary: PathBuf,
    pub changed: bool,
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut text: OsString = path.as_os_str().to_owned();
    text.push(suffix);
    PathBuf::from(text)
}

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
                "build it from source: `cargo install --locked --git https://github.com/aldesantis/ambit`",
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
                "curl -fsSL https://raw.githubusercontent.com/aldesantis/ambit/main/install.sh | sh",
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

/// Windows cannot replace a running executable but can rename it, so the old binary is moved
/// aside first; it cannot be deleted while running and is swept up on the next run.
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

fn missing_binary(asset: &str, reason: impl Into<String>) -> AmbitError {
    network_error(
        format!("{asset} does not contain the ambit binary"),
        [
            reason.into(),
            "report it at https://github.com/aldesantis/ambit/issues".to_owned(),
        ],
    )
}

fn is_binary_member(member: &str, root_dir: &str, binary_name: &str) -> bool {
    let member = member.strip_prefix("./").unwrap_or(member);

    member == binary_name || member.strip_prefix(root_dir) == Some(binary_name)
}

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

pub fn apply_self_update(plan: &SelfUpdatePlan, context: &SelfContext<'_>) -> Result<()> {
    let windows = context.os == "windows";
    let incoming = with_suffix(&plan.binary, INCOMING_SUFFIX);
    let download = with_suffix(&plan.binary, DOWNLOAD_SUFFIX);

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
                    "try again; if it keeps happening, report it at https://github.com/aldesantis/ambit/issues"
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

pub fn is_upgrade(plan: &SelfUpdatePlan) -> bool {
    is_newer(&plan.current, &plan.target)
}

#[cfg(test)]
mod tests;
