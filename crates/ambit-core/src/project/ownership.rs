//! Ownership enforcement: the safety core.
//!
//! ambit overwrites only what `.ambit/state.json` says it created. Anything else at a target path
//! belongs to someone else: a hand-written skill, a server added to `.mcp.json` before ambit ran, a
//! directory another tool maintains. This lets ambit be pointed at a project that already has
//! content.
//!
//! The check runs over the whole plan before any adapter writes anything, so a refusal leaves the
//! project exactly as it was rather than half-installed. `--adopt` is expressed by handing `apply`
//! a state that already owns the adopted target, so the target is replaced the way an owned one is,
//! instead of copied on top of and left carrying files the catalog no longer ships.
//!
//! Granularity follows the artifact. A skill directory is owned as a path. A harness config file is
//! co-owned: only ambit's keys inside it are ambit's, so a `.mcp.json` full of hand-added servers
//! is a normal input, and only a colliding server name is a conflict.

use std::io;
use std::path::{Path, PathBuf};

use indexmap::IndexSet;

use crate::errors::{AmbitError, Result, config_error};
use crate::harness::adapter::{PlannedArtifact, PlannedHarnessConfig};
use crate::harness::profile::{SHARED_SKILLS_DIR, holds_only_owned};
use crate::model::documents::{DocumentShape, driver_for, managed_key, read_document_text};
use crate::model::state::{ArtifactKind, OwnedArtifact, State, owned_paths};
use crate::util::fs::{self, EntryKind};

/// How an install was told to treat a target ambit does not own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnershipOptions {
    /// `--adopt`: take ownership of what is already there instead of refusing it.
    pub adopt: bool,
}

/// Whether anything at all sits at `target`.
///
/// `lstat`, not `stat`: a symlink (even a dangling one) is something ambit did not create and must
/// not silently replace, and a symlink is a shape ambit installs in its own right.
///
/// # Errors
///
/// Exit 2 when the path cannot be inspected. "I could not look" is not the same answer as "nothing
/// is there", and guessing the second would be guessing in the one direction that destroys data.
fn exists(target: &Path, file: &str) -> Result<bool> {
    match fs::lstat_kind(target) {
        Ok(EntryKind::Missing) => Ok(false),
        Ok(_) => Ok(true),
        Err(error) => Err(config_error(
            format!("cannot inspect {file}"),
            [
                fs::io_message(&error, "lstat", target),
                format!(
                    "make {} readable, so ambit can tell whether it would overwrite something",
                    target.display()
                ),
            ],
        )),
    }
}

/// `stat`, following links: whether the path resolves to a directory, `None` when nothing resolves
/// there.
fn stat_is_dir(path: &Path) -> io::Result<Option<bool>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.is_dir())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The first directory an artifact needs that something other than a directory occupies.
///
/// `mkdir -p` cannot create a path whose ancestor is a dangling symlink (`ENOENT`) or a file
/// (`ENOTDIR`). Both mean something ambit did not create is in the way. Left to surface inside
/// `apply`, they'd show as "unexpected internal error … this is a bug in ambit", so they are caught
/// here, before anything is written, naming the ancestor (which is what has to move) rather than
/// the artifact.
///
/// Returns the offending ancestor, project-relative, or `None` when the path is clear. A missing
/// ancestor is clear: `mkdir -p` creates it.
///
/// # Errors
///
/// Exit 2 when an ancestor cannot be inspected at all.
fn blocking_ancestor(path: &str, target: &Path) -> Result<Option<String>> {
    let segments: Vec<&str> = path.split('/').collect();

    // Ancestors as absolute paths, outermost first, walked up from the target rather than down
    // from a project root: the plan already carries both forms of this location.
    let mut ancestors: Vec<PathBuf> = Vec::new();
    let mut absolute = target.to_path_buf();

    for _ in 1..segments.len() {
        absolute = absolute
            .parent()
            .map_or(absolute.clone(), Path::to_path_buf);
        ancestors.insert(0, absolute.clone());
    }

    for (index, directory) in ancestors.iter().enumerate() {
        let walked = segments[..=index].join("/");

        // `stat`, following links: an ancestor that is a symlink to a real directory is a
        // directory as far as writing into it goes.
        match stat_is_dir(directory) {
            Ok(Some(true)) => {}
            Ok(Some(false)) => return Ok(Some(walked)),
            Ok(None) => {
                // Nothing resolves there: either the path is genuinely absent, or a link points at
                // something that is not there. Only `lstat` can tell those apart.
                match fs::lstat_kind(directory) {
                    Ok(EntryKind::Missing) | Err(_) => {}
                    Ok(_) => return Ok(Some(walked)),
                }
            }
            Err(error) => {
                return Err(config_error(
                    format!("cannot inspect {walked}"),
                    [
                        fs::io_message(&error, "stat", directory),
                        format!(
                            "make {} readable, so ambit can tell whether it can write beneath it",
                            directory.display()
                        ),
                    ],
                ));
            }
        }
    }

    Ok(None)
}

/// Deliberately does not offer `--adopt`: adoption governs what ambit may overwrite, and no amount
/// of it lets `mkdir` descend through a dangling link. Exit 2.
fn refuse_ancestor(path: &str, ancestor: &str) -> AmbitError {
    config_error(
        "refusing to write under an unowned path",
        [
            format!(
                "{ancestor} is not a directory ambit can write into, so {path} cannot be created"
            ),
            format!("move {ancestor} aside, or point it at a directory that exists"),
        ],
    )
}

/// Exit 2, in the standard wording for this case.
fn refuse_path(artifact: &PlannedArtifact) -> AmbitError {
    let detail = if artifact.kind() == ArtifactKind::SkillsLink {
        format!(
            "{} exists but ambit did not create it, so it cannot be pointed at {SHARED_SKILLS_DIR}",
            artifact.path()
        )
    } else {
        format!("{} exists but ambit did not create it", artifact.path())
    };

    config_error(
        "refusing to overwrite unowned path",
        [
            detail,
            "move it aside, or run `ambit install --adopt` to take ownership".to_owned(),
        ],
    )
}

/// Exit 2. Says "remove", not "move aside": the file itself stays put.
fn refuse_key(artifact: &PlannedHarnessConfig, key: &str) -> AmbitError {
    config_error(
        "refusing to overwrite unowned key",
        [
            format!(
                "\"{key}\" in {} exists but ambit did not create it",
                artifact.path
            ),
            format!(
                "remove it from {}, or run `ambit install --adopt` to take ownership",
                artifact.path
            ),
        ],
    )
}

/// The dotted keys prior state records as ambit's within one config file.
///
/// Unioned across every artifact naming that path, so ownership survives two adapters writing into
/// one file: the path alone never grants it, because the file is co-owned.
pub fn owned_keys(prior: &State, file: &str) -> IndexSet<String> {
    prior
        .artifacts
        .iter()
        .filter(|artifact| artifact.path == file)
        .flat_map(|artifact| artifact.managed_keys.iter().flatten().cloned())
        .collect()
}

/// The planned keys of one config file that the file already holds and ambit does not own, in
/// plan order.
///
/// Driven by the plan's entries, which arrive sorted, so which collision is reported first depends
/// on the bundle, not on the order keys happen to sit in the file.
///
/// # Errors
///
/// Exit 2 for a document that cannot be read at all. A section holding something other than an
/// object is left to the merge to report, which is the code that cannot proceed with it.
fn colliding_keys(artifact: &PlannedHarnessConfig, prior: &State) -> Result<Vec<String>> {
    let driver = driver_for(
        artifact.format,
        artifact.shape.unwrap_or(DocumentShape::Map),
        None,
    )?;
    let text = read_document_text(&artifact.target, &artifact.path)?;
    let present = driver.section_keys(text.as_deref(), &artifact.section, &artifact.path)?;

    if present.is_empty() {
        return Ok(Vec::new());
    }

    let owned = owned_keys(prior, &artifact.path);

    Ok(artifact
        .entries
        .iter()
        .filter(|entry| present.contains(&entry.key))
        .map(|entry| managed_key(&artifact.section, &entry.key))
        .filter(|key| !owned.contains(key))
        .collect())
}

/// What stands in the way of one whole-path artifact.
enum PathCheck {
    /// Nothing, or something ambit already owns.
    Clear,
    /// An ancestor that is not a directory ambit can write into, project-relative.
    Ancestor(String),
    /// Something ambit did not create, which only adoption may replace.
    Unowned,
    /// A pre-shared-layout skills directory holding only skills ambit installed, which is adopted
    /// without being asked.
    Migrating,
}

/// Checks one whole-path artifact against prior ownership and the filesystem.
///
/// # Errors
///
/// Exit 2 when the target or one of its ancestors cannot be inspected.
fn check_path(artifact: &PlannedArtifact, owned: &IndexSet<String>) -> Result<PathCheck> {
    let path = artifact.path();

    // Checked before the ownership question, regardless of what state says: an artifact ambit
    // owns is no more writable than a new one when the directory it lives in has been replaced by
    // a dangling link.
    if let Some(blocking) = blocking_ancestor(path, artifact.target())? {
        return Ok(PathCheck::Ancestor(blocking));
    }

    if owned.contains(path) || !exists(artifact.target(), path)? {
        return Ok(PathCheck::Clear);
    }

    // The one case adoption is implicit: a skills directory holding nothing but skills ambit
    // itself installed. This is what a pre-shared-layout install leaves behind, and replacing it
    // with a link to the shared directory loses nothing, since ambit wrote everything in it. One
    // hand-written skill in there and this is false, so the refusal stands.
    if artifact.kind() == ArtifactKind::SkillsLink
        && holds_only_owned(artifact.target(), path, owned)?
    {
        return Ok(PathCheck::Migrating);
    }

    Ok(PathCheck::Unowned)
}

/// Checks a whole plan against prior ownership, and returns the ownership `apply` may act with.
///
/// Call this once, with every adapter's plan, before any adapter runs, so a project with one
/// conflict is left untouched rather than partly written. `plan` is every artifact the run intends
/// to write; `prior` is the state from the last install, what ambit already owns.
///
/// Returns `prior`, plus an owned entry for every target `--adopt` just took over, so `apply`
/// replaces an adopted directory instead of copying into it and leaving strangers' files behind.
///
/// Adoption needs no bookkeeping for a config key: `apply` writes managed keys unconditionally,
/// precisely because the file is co-owned, so allowing the collision is the whole of taking it
/// over.
///
/// # Errors
///
/// Exit 2 naming the path or key it will not overwrite, and `--adopt` as the way to say otherwise.
pub fn authorize_plan(
    plan: &[PlannedArtifact],
    prior: &State,
    options: OwnershipOptions,
) -> Result<State> {
    let owned = owned_paths(prior);
    let mut adopted: Vec<OwnedArtifact> = Vec::new();

    for artifact in plan {
        if let PlannedArtifact::HarnessConfig(config) = artifact {
            // Read even under `--adopt`, so a document that cannot be parsed stops the run here
            // rather than halfway through `apply`.
            let colliding = colliding_keys(config, prior)?;

            if let Some(key) = colliding.first().filter(|_| !options.adopt) {
                return Err(refuse_key(config, key));
            }

            continue;
        }

        let path = artifact.path();

        match check_path(artifact, &owned)? {
            PathCheck::Clear => continue,
            PathCheck::Ancestor(blocking) => return Err(refuse_ancestor(path, &blocking)),
            PathCheck::Unowned if !options.adopt => return Err(refuse_path(artifact)),
            PathCheck::Unowned | PathCheck::Migrating => {}
        }

        adopted.push(OwnedArtifact {
            path: path.to_owned(),
            kind: artifact.kind(),
            mode: artifact.mode(),
            managed_keys: None,
            format: None,
            shape: None,
        });
    }

    if adopted.is_empty() {
        return Ok(prior.clone());
    }

    let mut authorized = prior.clone();

    authorized.artifacts.extend(adopted);
    Ok(authorized)
}

/// The summary of the error an operation that will not adopt returns for its conflicts.
///
/// Exported so the app's bindings can tell this refusal apart from other config errors.
pub const OWNERSHIP_CONFLICT: &str = "refusing to overwrite what ambit does not own";

/// One path or config key a plan would overwrite that ambit does not own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnershipConflict {
    /// Project-relative, `/`-separated: what has to move. For a blocked ancestor, the ancestor.
    pub path: String,
    /// The dotted managed key, for a conflict inside a co-owned config file.
    pub key: Option<String>,
    /// One line naming what is in the way.
    pub message: String,
    /// Why, ending in the corrective action. Never adoption.
    pub detail: Vec<String>,
}

/// Every conflict a plan has with prior ownership, without adopting anything.
///
/// The checks of [`authorize_plan`] with adoption off, collected rather than stopping at the
/// first, for a caller that shows every conflict at once and must never take a target over. The
/// skills-directory migration `authorize_plan` performs implicitly is not a conflict here either.
///
/// # Errors
///
/// Exit 2 when a target, an ancestor, or a config file cannot be inspected: "I could not look" is
/// not a conflict, and not the absence of one.
pub fn ownership_conflicts(
    plan: &[PlannedArtifact],
    prior: &State,
) -> Result<Vec<OwnershipConflict>> {
    let owned = owned_paths(prior);
    let mut conflicts = Vec::new();

    for artifact in plan {
        if let PlannedArtifact::HarnessConfig(config) = artifact {
            for key in colliding_keys(config, prior)? {
                conflicts.push(OwnershipConflict {
                    path: config.path.clone(),
                    message: format!("\"{key}\" in {} is not managed by ambit", config.path),
                    detail: vec![
                        format!(
                            "\"{key}\" in {} exists but ambit did not create it",
                            config.path
                        ),
                        format!(
                            "remove \"{key}\" from {}, or deselect what installs it",
                            config.path
                        ),
                    ],
                    key: Some(key),
                });
            }

            continue;
        }

        let path = artifact.path();

        match check_path(artifact, &owned)? {
            PathCheck::Clear | PathCheck::Migrating => {}
            PathCheck::Ancestor(blocking) => conflicts.push(OwnershipConflict {
                message: format!("{blocking} is in the way of {path}"),
                detail: vec![
                    format!(
                        "{blocking} is not a directory ambit can write into, so {path} cannot be created"
                    ),
                    format!("move {blocking} aside, or point it at a directory that exists"),
                ],
                path: blocking,
                key: None,
            }),
            PathCheck::Unowned => conflicts.push(OwnershipConflict {
                path: path.to_owned(),
                key: None,
                message: format!("{path} is not managed by ambit"),
                detail: vec![
                    format!("{path} exists but ambit did not create it"),
                    format!("move {path} aside, or deselect what installs it"),
                ],
            }),
        }
    }

    Ok(conflicts)
}

/// The error for a non-empty list of conflicts: exit 2, one detail line per conflict.
pub fn conflicts_error(conflicts: &[OwnershipConflict]) -> AmbitError {
    config_error(
        OWNERSHIP_CONFLICT,
        conflicts.iter().map(|conflict| {
            let step = conflict.detail.last().map_or("", String::as_str);

            format!("{}: {step}", conflict.message)
        }),
    )
}
