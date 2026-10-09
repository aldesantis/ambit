use std::path::Path;

use indexmap::{IndexMap, IndexSet};

use crate::errors::{Result, config_error};
use crate::harness::adapter::PlannedArtifact;
use crate::model::documents::{DocumentFormat, DocumentShape, driver_for, read_document_text};
use crate::model::state::{ArtifactKind, OwnedArtifact, STATE_DIRNAME, STATE_FILENAME, State};
use crate::util::cmp::js_cmp;
use crate::util::fs::{self, EntryKind};
use crate::util::path::join;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrunedArtifact {
    pub path: String,
    pub kind: ArtifactKind,
    pub managed_keys: Option<Vec<String>>,
    pub format: Option<DocumentFormat>,
    pub shape: Option<DocumentShape>,
}

fn planned_paths(plan: &[PlannedArtifact]) -> IndexSet<&str> {
    plan.iter()
        .filter(|artifact| artifact.kind() != ArtifactKind::HarnessConfig)
        .map(PlannedArtifact::path)
        .collect()
}

fn planned_keys(plan: &[PlannedArtifact]) -> IndexMap<&str, IndexSet<&str>> {
    let mut by_file: IndexMap<&str, IndexSet<&str>> = IndexMap::new();

    for artifact in plan {
        if let PlannedArtifact::HarnessConfig(config) = artifact {
            by_file
                .entry(config.path.as_str())
                .or_default()
                .extend(config.managed_keys.iter().map(String::as_str));
        }
    }

    by_file
}

fn split_managed_key<'k>(key: &'k str, file: &str) -> Result<(&'k str, &'k str)> {
    // First dot: entity names may contain dots (`mcpServers.acme.internal`).
    match key.find('.') {
        Some(dot) if dot > 0 && dot != key.len() - 1 => Ok((&key[..dot], &key[dot + 1..])),
        _ => Err(config_error(
            format!("cannot prune \"{key}\" from {file}"),
            [
                format!(
                    "{STATE_DIRNAME}/{STATE_FILENAME} records it as a managed key, but it names no section"
                ),
                format!(
                    "correct that entry, or delete {STATE_DIRNAME}/{STATE_FILENAME} and run `ambit install --adopt`"
                ),
            ],
        )),
    }
}

pub fn plan_prune(plan: &[PlannedArtifact], prior: &State) -> Result<Vec<PrunedArtifact>> {
    let kept_paths = planned_paths(plan);
    let kept_keys = planned_keys(plan);
    let no_keys = IndexSet::new();
    let mut stale = Vec::new();

    let mut artifacts: Vec<&OwnedArtifact> = prior.artifacts.iter().collect();

    artifacts.sort_by(|a, b| js_cmp(&a.path, &b.path));

    for artifact in artifacts {
        if artifact.kind == ArtifactKind::HarnessConfig {
            let kept = kept_keys.get(artifact.path.as_str()).unwrap_or(&no_keys);
            let mut keys: Vec<String> = artifact.managed_keys.clone().unwrap_or_default();

            keys.sort_by(|a, b| js_cmp(a, b));
            keys.retain(|key| !kept.contains(key.as_str()));

            if keys.is_empty() {
                continue;
            }

            for key in &keys {
                split_managed_key(key, &artifact.path)?;
            }

            stale.push(PrunedArtifact {
                path: artifact.path.clone(),
                kind: artifact.kind,
                managed_keys: Some(keys),
                format: artifact.format,
                shape: artifact.shape,
            });
            continue;
        }

        if kept_paths.contains(artifact.path.as_str()) {
            continue;
        }

        stale.push(PrunedArtifact {
            path: artifact.path.clone(),
            kind: artifact.kind,
            managed_keys: None,
            format: None,
            shape: None,
        });
    }

    Ok(stale)
}

pub fn remaining_artifacts(prior: &State, pruned: &[PrunedArtifact]) -> Vec<OwnedArtifact> {
    let mut removed: IndexMap<&str, IndexSet<&str>> = IndexMap::new();

    for artifact in pruned {
        removed
            .entry(artifact.path.as_str())
            .or_default()
            .extend(artifact.managed_keys.iter().flatten().map(String::as_str));
    }

    let mut kept = Vec::new();

    for artifact in &prior.artifacts {
        let Some(gone_keys) = removed.get(artifact.path.as_str()) else {
            kept.push(artifact.clone());
            continue;
        };

        if artifact.kind != ArtifactKind::HarnessConfig {
            continue;
        }

        let keys: Vec<String> = artifact
            .managed_keys
            .iter()
            .flatten()
            .filter(|key| !gone_keys.contains(key.as_str()))
            .cloned()
            .collect();

        if !keys.is_empty() {
            kept.push(OwnedArtifact {
                managed_keys: Some(keys),
                ..artifact.clone()
            });
        }
    }

    kept
}

fn prune_config_keys(
    project_dir: &Path,
    file: &str,
    stale: &[String],
    format: DocumentFormat,
    shape: Option<DocumentShape>,
) -> Result<Option<PrunedArtifact>> {
    let target = join(project_dir, file);
    let driver = driver_for(format, shape.unwrap_or(DocumentShape::Map), None)?;
    // Re-read: `apply` already merged this run's keys, so a planning-time snapshot would undo them.
    let mut text = read_document_text(&target, file)?;
    let mut removed = Vec::new();

    for key in stale {
        let (section, name) = split_managed_key(key, file)?;
        let Some(next) = driver.remove_keys(text.as_deref(), section, &[name.to_owned()], file)?
        else {
            continue;
        };

        text = Some(next);
        removed.push(key.clone());
    }

    let Some(text) = text.filter(|_| !removed.is_empty()) else {
        return Ok(None);
    };

    fs::write_text(&target, &text)?;

    Ok(Some(PrunedArtifact {
        path: file.to_owned(),
        kind: ArtifactKind::HarnessConfig,
        managed_keys: Some(removed),
        format: None,
        shape: None,
    }))
}

fn owned_path_intact(project_dir: &Path, relative: &str) -> bool {
    let segments: Vec<&str> = relative.split('/').collect();
    let mut current = project_dir.to_path_buf();

    for segment in &segments[..segments.len() - 1] {
        current = join(&current, segment);

        match fs::lstat_kind(&current) {
            // An old `.claude/skills/<name>` now resolves through the link into the new install.
            Ok(EntryKind::Symlink) => return false,
            Ok(EntryKind::Missing) | Err(_) => return true,
            Ok(_) => {}
        }
    }

    true
}

/// Pass every adapter's plan at once: pruning per adapter deletes what another adapter now writes.
pub fn prune_artifacts(
    project_dir: &Path,
    plan: &[PlannedArtifact],
    prior: &State,
) -> Result<Vec<PrunedArtifact>> {
    let mut pruned = Vec::new();

    for artifact in plan_prune(plan, prior)? {
        if artifact.kind == ArtifactKind::HarnessConfig {
            let removed = prune_config_keys(
                project_dir,
                &artifact.path,
                artifact.managed_keys.as_deref().unwrap_or_default(),
                artifact.format.unwrap_or(DocumentFormat::Json),
                artifact.shape,
            )?;

            pruned.extend(removed);
            continue;
        }

        if owned_path_intact(project_dir, &artifact.path) {
            fs::rm_rf(&join(project_dir, &artifact.path))?;
        }

        pruned.push(artifact);
    }

    Ok(pruned)
}
