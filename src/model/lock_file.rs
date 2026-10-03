//! `ambit.lock` as a file ambit reads: where it lives, and the pins inside it.
//!
//! The writing half is `project/lock.rs`, which builds the document and renders its bytes.
//! Rendering a lock needs a resolved bundle and its reasons (project-level knowledge); reading a
//! pin back out is something catalog loading has to do, so it lives beside the loader that needs
//! it.
//!
//! Why a lock is read at all: resolving a moving `ref:` from the machine-wide git cache alone would
//! give a project whatever the shared clone happened to hold, which any other project on the
//! machine could move under it. That would make `--frozen` unsatisfiable for a project using
//! `ref: main`: a cold CI clone resolves `main` to today's commit and fails against a lock written
//! last week.
//!
//! So the `catalogs` section is an input, resolved against rather than just recorded. Every other
//! section stays a record, compared as bytes rather than consumed; see `project/lock.rs`.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::{AmbitError, Result, config_error};
use crate::model::config::ProjectConfig;
use crate::model::git::is_commit_sha;
use crate::model::sources::{Source, SourceRequest, parse_source};
use crate::model::yaml::parse_yaml_mapping;
use crate::util::fs::{io_message, read_text_opt};
use crate::util::path::join;

/// The lockfile's name, at the project root beside `ambit.yml`.
pub const LOCK_FILENAME: &str = "ambit.lock";

/// The only lock version this build reads or writes.
pub const LOCK_VERSION: i64 = 1;

/// Where the lock lives for a project.
pub fn lock_file_path(project_dir: &Path) -> PathBuf {
    join(project_dir, LOCK_FILENAME)
}

/// Reads a project's lock as text, returning `None` when there is none.
///
/// Text, because that is what `--frozen` compares: a lock that would be rewritten is out of date,
/// whatever the two documents mean.
///
/// # Errors
///
/// Exit 2 for a lock that exists but cannot be read: reported rather than treated as absent, since
/// "there is no lock" and "your lock is unreadable" call for different fixes.
pub fn read_lock_text(project_dir: &Path) -> Result<Option<String>> {
    let file = lock_file_path(project_dir);

    read_text_opt(&file).map_err(|error| {
        config_error(
            format!("cannot read {LOCK_FILENAME}"),
            [
                io_message(&error, "open", &file),
                format!(
                    "make {} readable, or delete it and run `ambit install` again",
                    file.display()
                ),
            ],
        )
    })
}

/// One `catalogs` entry as the lock recorded it: what it was resolved from, and what it resolved
/// to.
struct RecordedCatalog {
    /// The `source` as config wrote it when the commit was recorded.
    source: String,
    /// The `ref` as config wrote it, absent when the entry named none.
    r#ref: Option<String>,
    /// The commit the ref resolved to. Absent for a `path:` source, which has no revision.
    commit: Option<String>,
}

/// The error for a lock this build cannot read a pin out of.
fn unsupported_version(found: i64) -> AmbitError {
    config_error(
        format!("{LOCK_FILENAME} is version {found}, which this build cannot read"),
        [
            format!(
                "ambit resolves against the commits a lock records, and only version {LOCK_VERSION} is a shape it knows"
            ),
            format!("upgrade ambit, or delete {LOCK_FILENAME} and run `ambit install` again"),
        ],
    )
}

/// Reads the `catalogs` section back.
///
/// Failures are fatal. A lock is an input, so a version this build does not know and a document
/// that does not parse both mean ambit cannot tell what this project is pinned to. Resolving as
/// though there were no lock would reintroduce the silent drift the pins exist to remove.
///
/// Unknown keys are deliberately not rejected, unlike everywhere else ambit parses YAML: only these
/// three are read, and a lock written by a later ambit that records a fourth should still pin
/// correctly rather than refuse to be read at all.
///
/// # Errors
///
/// Exit 2 for an unreadable lock, a version this build cannot read, a malformed document, or a
/// `commit` that is not a full SHA.
fn read_recorded_catalogs(project_dir: &Path) -> Result<Option<IndexMap<String, RecordedCatalog>>> {
    let Some(text) = read_lock_text(project_dir)? else {
        return Ok(None);
    };

    let root = parse_yaml_mapping(&text, LOCK_FILENAME)?;
    let version = root.require_integer("version")?;

    if version != LOCK_VERSION {
        return Err(unsupported_version(version));
    }

    let Some(catalogs) = root.optional_mapping("catalogs")? else {
        return Ok(Some(IndexMap::new()));
    };

    let mut recorded = IndexMap::new();

    for name in catalogs.keys() {
        let entry = catalogs.require_mapping(&name)?;
        let commit = entry.optional_string("commit")?;
        let r#ref = entry.optional_string("ref")?;

        // Refused here rather than left to git, so the message names the file the pin was
        // hand-edited in.
        if let Some(commit) = &commit
            && !is_commit_sha(commit)
        {
            return Err(entry.key_error(
                "commit",
                &format!("catalog \"{name}\" is pinned to something that is not a commit"),
                vec![
                    format!("\"{commit}\" is not a full commit SHA"),
                    format!(
                        "delete {LOCK_FILENAME} and run `ambit install` again to write a correct one"
                    ),
                ],
            ));
        }

        let source = entry.require_string("source")?;

        recorded.insert(
            name,
            RecordedCatalog {
                source,
                r#ref,
                commit,
            },
        );
    }

    Ok(Some(recorded))
}

/// What a `source`/`ref` pair means, as one comparable string, or nothing if it means nothing here.
///
/// Parsed rather than compared as written, because the question a pin's validity turns on is
/// whether this is still the same repository at the same revision, and one repository has several
/// spellings: `acme/skills` and `https://github.com/acme/skills.git` are one source, as are a URL
/// and its `git:` form, and `acme/skills@v1` says what a separate `ref: v1` says. Comparing the
/// strings would void a good pin over a rewrite that changed nothing, sending the run to the
/// network to rediscover a commit it already had.
///
/// A source that does not parse is not comparable, so its pin is void rather than honoured, as is a
/// `path:` source, which has no revision to pin in the first place. Nothing is returned as an
/// error here: the config's own source is about to be parsed properly by the load that follows, and
/// a bad source in the lock is a pin to ignore rather than a project to stop.
fn git_identity(source: &str, r#ref: Option<&str>) -> Option<String> {
    let request = SourceRequest {
        source: source.to_owned(),
        r#ref: r#ref.map(str::to_owned),
        ..SourceRequest::default()
    };

    match parse_source(&request) {
        Ok(Source::Git { url, r#ref }) => Some(format!("{url} {}", r#ref.unwrap_or_default())),
        Ok(Source::Path { .. }) | Err(_) => None,
    }
}

/// The commit each configured catalog is pinned to, keyed by catalog name.
///
/// Empty for a project with no lock, since a project with nothing to reproduce should resolve
/// against its remote rather than inherit a shared clone's idea of `main`.
///
/// An entry survives only when the lock's `source` and `ref` still name the same repository and
/// revision `ambit.yml` does (see `git_identity`), and only when it has a commit at all. Three
/// cases drop it:
///
/// - The config moved: `ref:` was edited, or `source:` repointed. The recorded commit answers a
///   question the project has stopped asking, so it is dropped and the new `ref` is resolved.
/// - The catalog is new: added since the lock was written, so it resolves against its remote
///   exactly as a first install's catalogs do.
/// - `path:`: no revision, so nothing to pin.
///
/// # Errors
///
/// Exit 2 for a lock that exists and cannot be read, a version this build cannot read, a malformed
/// document, or a `commit` that is not a full SHA.
pub fn read_catalog_pins(
    project_dir: &Path,
    config: &ProjectConfig,
) -> Result<IndexMap<String, String>> {
    let Some(recorded) = read_recorded_catalogs(project_dir)? else {
        return Ok(IndexMap::new());
    };

    let mut pins = IndexMap::new();

    for entry in &config.catalogs {
        let Some(locked) = recorded.get(&entry.name) else {
            continue;
        };

        let Some(commit) = &locked.commit else {
            continue;
        };

        let Some(configured) = git_identity(&entry.source, entry.r#ref.as_deref()) else {
            continue;
        };

        if git_identity(&locked.source, locked.r#ref.as_deref()).as_ref() != Some(&configured) {
            continue;
        }

        pins.insert(entry.name.clone(), commit.clone());
    }

    Ok(pins)
}
