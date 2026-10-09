use std::io;
use std::path::{Path, PathBuf};

use indexmap::{IndexMap, IndexSet};

use crate::errors::{AmbitError, Result, config_error};
use crate::harness::adapter::{
    PlannedArtifact, PlannedCatalogDir, PlannedHarnessConfig, PlannedSkillsLink,
};
use crate::harness::profile::SHARED_SKILLS_DIR;
use crate::model::catalog::{CatalogLoadOptions, load_catalogs, merge_catalogs};
use crate::model::config::load_project_config;
use crate::model::documents::{DocumentShape, driver_for, managed_key, read_document_text};
use crate::model::sources::SourceContext;
use crate::model::state::{ArtifactKind, State, owned_paths, read_state};
use crate::project::install::{adapters_for, plan_for, project_paths};
use crate::project::ownership::owned_keys;
use crate::resolution::resolve::resolve_bundle;
use crate::util::cmp::js_cmp;
use crate::util::env::Env;
use crate::util::fs::{EntryKind, io_message, lstat_kind, read_dir_names};
use crate::util::hash::tree_digest;
use crate::util::path::{normalize, resolve, to_slash};
use crate::util::string_enum;

#[cfg(test)]
mod hooks_tests;
#[cfg(test)]
mod tests;

string_enum! {
    pub enum ArtifactState {
        Missing => "missing",
        Modified => "modified",
        Ok => "ok",
        Stale => "stale",
        Unowned => "unowned",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusArtifact {
    pub path: String,
    pub kind: ArtifactKind,
    pub state: ArtifactState,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectStatus {
    pub artifacts: Vec<StatusArtifact>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusOptions {
    pub offline: bool,
}

struct Verdict {
    state: ArtifactState,
    detail: String,
}

impl Verdict {
    const OK: Self = Self {
        state: ArtifactState::Ok,
        detail: String::new(),
    };

    fn new(state: ArtifactState, detail: impl Into<String>) -> Self {
        Self {
            state,
            detail: detail.into(),
        }
    }

    fn at(self, path: &str, kind: ArtifactKind) -> StatusArtifact {
        StatusArtifact {
            path: path.to_owned(),
            kind,
            state: self.state,
            detail: self.detail,
        }
    }
}

pub fn status_drift(status: &ProjectStatus) -> Vec<StatusArtifact> {
    status
        .artifacts
        .iter()
        .filter(|artifact| artifact.state != ArtifactState::Ok)
        .cloned()
        .collect()
}

pub fn is_clean(status: &ProjectStatus) -> bool {
    status_drift(status).is_empty()
}

fn unreadable(file: &str, target: &Path, message: String) -> AmbitError {
    config_error(
        format!("cannot inspect {file}"),
        [
            message,
            format!(
                "make {} readable, so ambit can tell whether it matches what it would install",
                target.display()
            ),
        ],
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Absent,
    Directory,
    Link,
    Other,
}

fn shape_of(target: &Path, file: &str) -> Result<Shape> {
    match lstat_kind(target) {
        Ok(EntryKind::Missing) => Ok(Shape::Absent),
        Ok(EntryKind::Symlink) => Ok(Shape::Link),
        Ok(EntryKind::Dir) => Ok(Shape::Directory),
        Ok(EntryKind::File | EntryKind::Other) => Ok(Shape::Other),
        Err(error) => Err(unreadable(file, target, io_message(&error, target))),
    }
}

fn file_list(dir: &Path, label: &str) -> Result<Vec<String>> {
    fn walk(
        current: &Path,
        relative: &str,
        found: &mut Vec<String>,
    ) -> std::result::Result<(), (io::Error, PathBuf)> {
        let names = read_dir_names(current).map_err(|e| (e, current.to_path_buf()))?;

        for name in names {
            let within = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let entry = current.join(&name);
            let kind = lstat_kind(&entry).map_err(|e| (e, entry.clone()))?;

            if kind == EntryKind::Dir {
                walk(&entry, &within, found)?;
            } else {
                found.push(within);
            }
        }

        Ok(())
    }

    let mut found = Vec::new();

    walk(dir, "", &mut found)
        .map_err(|(error, at)| unreadable(label, dir, io_message(&error, &at)))?;
    found.sort_by(|a, b| js_cmp(a, b));
    Ok(found)
}

fn same_bytes(source: &Path, target: &Path) -> bool {
    match (std::fs::read(source), std::fs::read(target)) {
        (Ok(expected), Ok(actual)) => expected == actual,
        _ => false,
    }
}

fn first_difference(artifact: &PlannedCatalogDir) -> Result<Option<String>> {
    let expected = file_list(
        &artifact.source,
        &format!("the source of \"{}\"", artifact.name),
    )?;
    let actual = file_list(&artifact.target, &artifact.path)?;
    let installed: IndexSet<&String> = actual.iter().collect();
    let shipped: IndexSet<&String> = expected.iter().collect();
    let mut all: Vec<&String> = expected
        .iter()
        .chain(&actual)
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect();
    all.sort_by(|a, b| js_cmp(a, b));

    for relative in all {
        if !installed.contains(relative) {
            return Ok(Some(format!("{relative} is missing")));
        }

        if !shipped.contains(relative) {
            return Ok(Some(format!("{relative} is not in its source")));
        }

        if !same_bytes(
            &artifact.source.join(relative),
            &artifact.target.join(relative),
        ) {
            return Ok(Some(format!("{relative} differs from its source")));
        }
    }

    Ok(None)
}

fn digest_of(dir: &Path, label: &str) -> Result<String> {
    tree_digest(dir).map_err(|error| unreadable(label, dir, io_message(&error, dir)))
}

fn copy_verdict(artifact: &PlannedCatalogDir, recorded: Option<&str>) -> Result<Verdict> {
    let Some(recorded) = recorded else {
        return Ok(match first_difference(artifact)? {
            None => Verdict::OK,
            Some(difference) => Verdict::new(ArtifactState::Modified, difference),
        });
    };

    let installed = digest_of(&artifact.target, &artifact.path)?;
    let source = digest_of(
        &artifact.source,
        &format!("the source of \"{}\"", artifact.name),
    )?;

    if installed == source {
        return Ok(Verdict::OK);
    }

    if let Some(difference) = first_difference(artifact)? {
        return Ok(Verdict::new(ArtifactState::Modified, difference));
    }

    if installed == recorded {
        return Ok(Verdict::OK);
    }

    Ok(Verdict::new(
        ArtifactState::Modified,
        "its contents changed since ambit installed it",
    ))
}

fn link_verdict(path: &str, target: &Path, source: &Path) -> Result<Verdict> {
    let written = std::fs::read_link(target)
        .map_err(|error| unreadable(path, target, io_message(&error, target)))?;
    let written = written.to_string_lossy();

    // Deliberately not `canonicalize`: only the link itself is resolved, not symlinks above it.
    let parent = target.parent().unwrap_or(target);
    let points = resolve(parent, &written);

    if points == normalize(source) {
        return Ok(Verdict::OK);
    }

    Ok(Verdict::new(
        ArtifactState::Modified,
        format!(
            "it points at {}, not at its source",
            to_slash(Path::new(&*written))
        ),
    ))
}

fn skills_link_verdict(artifact: &PlannedSkillsLink, owned: &IndexSet<String>) -> Result<Verdict> {
    let shape = shape_of(&artifact.target, &artifact.path)?;

    if shape == Shape::Absent {
        return Ok(Verdict::new(
            ArtifactState::Missing,
            "nothing is installed at this path",
        ));
    }

    if !owned.contains(&artifact.path) {
        return Ok(Verdict::new(
            ArtifactState::Unowned,
            "it exists but ambit did not create it",
        ));
    }

    if shape == Shape::Link {
        return link_verdict(&artifact.path, &artifact.target, &artifact.source);
    }

    Ok(Verdict::new(
        ArtifactState::Modified,
        format!("it is not a symlink to {SHARED_SKILLS_DIR}"),
    ))
}

fn catalog_dir_verdict(
    artifact: &PlannedCatalogDir,
    owned: &IndexSet<String>,
    recorded: Option<&str>,
) -> Result<Verdict> {
    let shape = shape_of(&artifact.target, &artifact.path)?;

    if shape == Shape::Absent {
        return Ok(Verdict::new(
            ArtifactState::Missing,
            "nothing is installed at this path",
        ));
    }

    if !owned.contains(&artifact.path) {
        return Ok(Verdict::new(
            ArtifactState::Unowned,
            "it exists but ambit did not create it",
        ));
    }

    if shape == Shape::Link {
        return link_verdict(&artifact.path, &artifact.target, &artifact.source);
    }

    if shape == Shape::Other {
        return Ok(Verdict::new(
            ArtifactState::Modified,
            "it is not a directory",
        ));
    }

    copy_verdict(artifact, recorded)
}

fn config_verdict(
    artifacts: &[&PlannedHarnessConfig],
    file: &str,
    target: &Path,
    claimed: &IndexSet<String>,
    stale: &[String],
) -> Result<Verdict> {
    let text = read_document_text(target, file)?;

    for artifact in artifacts {
        let driver = driver_for(
            artifact.format,
            artifact.shape.unwrap_or(DocumentShape::Map),
            None,
        )?;
        let present = driver.section_keys(text.as_deref(), &artifact.section, file)?;

        for entry in &artifact.entries {
            let key = managed_key(&artifact.section, &entry.key);

            if !present.contains(&entry.key) {
                return Ok(Verdict::new(
                    ArtifactState::Missing,
                    format!("\"{key}\" is absent"),
                ));
            }

            if !claimed.contains(&key) {
                return Ok(Verdict::new(
                    ArtifactState::Unowned,
                    format!("\"{key}\" exists but ambit did not create it"),
                ));
            }

            if !driver.entry_matches(text.as_deref(), &artifact.section, entry, file)? {
                return Ok(Verdict::new(
                    ArtifactState::Modified,
                    format!("\"{key}\" is not what install would write"),
                ));
            }
        }
    }

    if let Some(first) = stale.first() {
        return Ok(Verdict::new(
            ArtifactState::Stale,
            format!("\"{first}\" is no longer selected"),
        ));
    }

    Ok(Verdict::OK)
}

fn planned_by_path(plan: &[PlannedArtifact]) -> IndexMap<&str, Vec<&PlannedArtifact>> {
    let mut by_path: IndexMap<&str, Vec<&PlannedArtifact>> = IndexMap::new();

    for artifact in plan {
        by_path.entry(artifact.path()).or_default().push(artifact);
    }

    by_path
}

fn planned_keys<'a>(artifacts: &[&'a PlannedHarnessConfig]) -> IndexSet<&'a str> {
    artifacts
        .iter()
        .flat_map(|artifact| artifact.managed_keys.iter().map(String::as_str))
        .collect()
}

fn compare_artifacts(plan: &[PlannedArtifact], prior: &State) -> Result<Vec<StatusArtifact>> {
    let owned = owned_paths(prior);
    let digests: IndexMap<&str, &str> = prior
        .artifacts
        .iter()
        .filter_map(|artifact| Some((artifact.path.as_str(), artifact.digest.as_deref()?)))
        .collect();
    let groups = planned_by_path(plan);
    let mut rows = Vec::new();

    for (&file, group) in &groups {
        let Some(&first) = group.first() else {
            continue;
        };

        match first {
            PlannedArtifact::SkillDir(dir) | PlannedArtifact::HookDir(dir) => {
                let recorded = digests.get(file).copied();

                rows.push(catalog_dir_verdict(dir, &owned, recorded)?.at(file, first.kind()));
            }
            PlannedArtifact::SkillsLink(link) => {
                rows.push(skills_link_verdict(link, &owned)?.at(file, first.kind()));
            }
            PlannedArtifact::HarnessConfig(_) => {
                let configs: Vec<&PlannedHarnessConfig> = group
                    .iter()
                    .filter_map(|artifact| match artifact {
                        PlannedArtifact::HarnessConfig(config) => Some(config),
                        _ => None,
                    })
                    .collect();
                let claimed = owned_keys(prior, file);
                let kept = planned_keys(&configs);
                let mut stale: Vec<String> = claimed
                    .iter()
                    .filter(|key| !kept.contains(key.as_str()))
                    .cloned()
                    .collect();
                stale.sort_by(|a, b| js_cmp(a, b));

                rows.push(
                    config_verdict(&configs, file, first.target(), &claimed, &stale)?
                        .at(file, first.kind()),
                );
            }
        }
    }

    let mut reported: IndexSet<&str> = IndexSet::new();

    for artifact in &prior.artifacts {
        if groups.contains_key(artifact.path.as_str()) || !reported.insert(&artifact.path) {
            continue;
        }

        rows.push(StatusArtifact {
            path: artifact.path.clone(),
            kind: artifact.kind,
            state: ArtifactState::Stale,
            detail: "ambit owns it, and nothing selects it now".to_owned(),
        });
    }

    rows.sort_by(|a, b| js_cmp(&a.path, &b.path));
    Ok(rows)
}

pub fn status_of_plan(plan: &[PlannedArtifact], prior: &State) -> Result<ProjectStatus> {
    Ok(ProjectStatus {
        artifacts: compare_artifacts(plan, prior)?,
    })
}

pub fn project_status(
    project_dir: &Path,
    env: &Env,
    options: StatusOptions,
) -> Result<ProjectStatus> {
    let config = load_project_config(project_dir)?;
    let mut harnesses: Vec<String> = config
        .harnesses
        .iter()
        .cloned()
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect();
    harnesses.sort_by(|a, b| js_cmp(a, b));
    let adapters = adapters_for(&harnesses)?;

    let context = SourceContext {
        project_dir: project_dir.to_path_buf(),
        env: env.clone(),
        offline: options.offline,
    };
    let catalogs = load_catalogs(&config, &context, &mut CatalogLoadOptions::default())?;
    let bundle = resolve_bundle(&config, &merge_catalogs(&catalogs))?;

    let project = project_paths(project_dir, env, None);
    let plan: Vec<PlannedArtifact> = plan_for(&adapters, &bundle, &project)
        .into_iter()
        .flat_map(|planned| planned.plan)
        .collect();

    status_of_plan(&plan, &read_state(project_dir)?)
}
