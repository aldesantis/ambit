use std::io;
use std::path::{Path, PathBuf};

use indexmap::IndexSet;

use crate::errors::{AmbitError, Result, config_error};
use crate::harness::adapter::{PlannedArtifact, PlannedHarnessConfig};
use crate::harness::profile::{SHARED_SKILLS_DIR, holds_only_owned};
use crate::model::documents::{DocumentShape, driver_for, managed_key, read_document_text};
use crate::model::state::{ArtifactKind, OwnedArtifact, State, owned_paths};
use crate::util::fs::{self, EntryKind};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnershipOptions {
    pub adopt: bool,
}

fn exists(target: &Path, file: &str) -> Result<bool> {
    match fs::lstat_kind(target) {
        Ok(EntryKind::Missing) => Ok(false),
        Ok(_) => Ok(true),
        Err(error) => Err(config_error(
            format!("cannot inspect {file}"),
            [
                fs::io_message(&error, target),
                format!(
                    "make {} readable, so ambit can tell whether it would overwrite something",
                    target.display()
                ),
            ],
        )),
    }
}

fn stat_is_dir(path: &Path) -> io::Result<Option<bool>> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata.is_dir())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn blocking_ancestor(path: &str, target: &Path) -> Result<Option<String>> {
    let segments: Vec<&str> = path.split('/').collect();

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

        match stat_is_dir(directory) {
            Ok(Some(true)) => {}
            Ok(Some(false)) => return Ok(Some(walked)),
            Ok(None) => match fs::lstat_kind(directory) {
                Ok(EntryKind::Missing) | Err(_) => {}
                Ok(_) => return Ok(Some(walked)),
            },
            Err(error) => {
                return Err(config_error(
                    format!("cannot inspect {walked}"),
                    [
                        fs::io_message(&error, directory),
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

pub fn owned_keys(prior: &State, file: &str) -> IndexSet<String> {
    prior
        .artifacts
        .iter()
        .filter(|artifact| artifact.path == file)
        .flat_map(|artifact| artifact.managed_keys.iter().flatten().cloned())
        .collect()
}

fn check_config_keys(
    artifact: &PlannedHarnessConfig,
    prior: &State,
    options: OwnershipOptions,
) -> Result<()> {
    let driver = driver_for(
        artifact.format,
        artifact.shape.unwrap_or(DocumentShape::Map),
        None,
    )?;
    let text = read_document_text(&artifact.target, &artifact.path)?;
    let present = driver.section_keys(text.as_deref(), &artifact.section, &artifact.path)?;

    if present.is_empty() || options.adopt {
        return Ok(());
    }

    let owned = owned_keys(prior, &artifact.path);

    for entry in &artifact.entries {
        let key = managed_key(&artifact.section, &entry.key);

        if present.contains(&entry.key) && !owned.contains(&key) {
            return Err(refuse_key(artifact, &key));
        }
    }

    Ok(())
}

pub fn authorize_plan(
    plan: &[PlannedArtifact],
    prior: &State,
    options: OwnershipOptions,
) -> Result<State> {
    let owned = owned_paths(prior);
    let mut adopted: Vec<OwnedArtifact> = Vec::new();

    for artifact in plan {
        if let PlannedArtifact::HarnessConfig(config) = artifact {
            check_config_keys(config, prior, options)?;
            continue;
        }

        let path = artifact.path();

        if let Some(blocking) = blocking_ancestor(path, artifact.target())? {
            return Err(refuse_ancestor(path, &blocking));
        }

        if owned.contains(path) {
            continue;
        }

        if !exists(artifact.target(), path)? {
            continue;
        }

        let migrating = artifact.kind() == ArtifactKind::SkillsLink
            && holds_only_owned(artifact.target(), path, &owned)?;

        if !migrating && !options.adopt {
            return Err(refuse_path(artifact));
        }

        adopted.push(OwnedArtifact {
            path: path.to_owned(),
            kind: artifact.kind(),
            mode: artifact.mode(),
            managed_keys: None,
            format: None,
            shape: None,
            digest: None,
        });
    }

    if adopted.is_empty() {
        return Ok(prior.clone());
    }

    let mut authorized = prior.clone();

    authorized.artifacts.extend(adopted);
    Ok(authorized)
}
