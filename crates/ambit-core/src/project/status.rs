//! `ambit status`: what is installed, against what resolution now produces.
//!
//! Install is idempotent: running it twice on an unchanged project moves no bytes. This command
//! checks that without touching anything: it plans exactly as install does, then compares the plan
//! against the project, so every row answers "would `ambit install` change this?" `--check` turns
//! the answer into exit 5 for CI.
//!
//! Nothing here writes, and nothing fails for drift; a project edited by hand is a state to
//! describe, not refuse. The errors that do escape are the ones resolution itself raises (a
//! malformed config, an unreachable catalog), since status cannot compare against a project it
//! cannot resolve.
//!
//! Comparison follows the artifact kind, matching the split ownership and pruning make: a copied
//! skill directory is compared as a tree of bytes; a symlink has none of its own, so only where it
//! points is checked (editing through the link edits the source, and is never drift); a harness
//! config file is co-owned, so it is compared key by key and only the keys ambit wrote are ambit's
//! to judge.
//!
//! Ownership is part of the comparison, not a separate audit: a target that exists but that state
//! does not claim is exactly what install would refuse, reported here as `unowned`.

use std::io;
use std::path::{Path, PathBuf};

use indexmap::{IndexMap, IndexSet};

use crate::errors::{AmbitError, Result, config_error};
use crate::harness::adapter::{
    PlannedArtifact, PlannedCatalogDir, PlannedHarnessConfig, PlannedSkillsLink, ProjectPaths,
};
use crate::harness::profile::SHARED_SKILLS_DIR;
use crate::model::catalog::{CatalogLoadOptions, load_catalogs, merge_catalogs};
use crate::model::config::load_project_config;
use crate::model::documents::{DocumentShape, driver_for, managed_key, read_document_text};
use crate::model::sources::SourceContext;
use crate::model::state::{ArtifactKind, State, owned_paths, read_state};
use crate::project::install::{adapters_for, install_scope, plan_for};
use crate::project::ownership::owned_keys;
use crate::resolution::resolve::resolve_bundle;
use crate::util::cmp::js_cmp;
use crate::util::env::Env;
use crate::util::fs::{EntryKind, io_message, lstat_kind, read_dir_names};
use crate::util::path::{normalize, resolve, to_slash};
use crate::util::string_enum;

#[cfg(all(test, feature = "cli"))]
mod hooks_tests;
#[cfg(all(test, feature = "cli"))]
mod tests;

string_enum! {
    /// What comparing one artifact against the project concluded.
    ///
    /// - `Missing`: resolution wants it and nothing is installed.
    /// - `Modified`: it is installed and owned, but its contents are not what install would write.
    /// - `Ok`: install would write exactly what is already there.
    /// - `Stale`: ambit owns it and resolution no longer selects it, so install would prune it.
    /// - `Unowned`: something is there that ambit did not create, which install refuses to
    ///   overwrite.
    pub enum ArtifactState {
        Missing => "missing",
        Modified => "modified",
        Ok => "ok",
        Stale => "stale",
        Unowned => "unowned",
    }
}

/// One artifact's verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusArtifact {
    /// Project-relative, `/`-separated.
    pub path: String,
    pub kind: ArtifactKind,
    pub state: ArtifactState,
    /// One line naming what differs, empty when `ok`.
    pub detail: String,
}

/// What `status` found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectStatus {
    /// Every artifact resolution wants plus every one state still owns, sorted by path.
    pub artifacts: Vec<StatusArtifact>,
}

/// How a status comparison was asked to behave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusOptions {
    /// Resolve from the catalog cache alone, failing rather than fetching.
    pub offline: bool,
}

/// A verdict before it is attached to a path: what every comparison below returns.
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

/// Everything `status` would report, which is everything install would change.
pub fn status_drift(status: &ProjectStatus) -> Vec<StatusArtifact> {
    status
        .artifacts
        .iter()
        .filter(|artifact| artifact.state != ArtifactState::Ok)
        .cloned()
        .collect()
}

/// Whether install would leave the project exactly as it is: the answer `--check` reports.
pub fn is_clean(status: &ProjectStatus) -> bool {
    status_drift(status).is_empty()
}

/// The error for a target that cannot be inspected.
///
/// Exit 2: "I could not look" is not a comparison result, and reporting it as drift would send
/// someone editing files over a permission problem instead.
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

/// What sits at a target: nothing, a symlink, a directory, or something else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Absent,
    Directory,
    Link,
    Other,
}

/// What sits at a target.
///
/// Uses `lstat`, not `stat`: a symlink is a legitimate install mode of its own. Following it would
/// compare a linked skill as though it were a copy, and would report a dangling link as absent when
/// it is actually there.
///
/// # Errors
///
/// Exit 2 when the path cannot be inspected.
fn shape_of(target: &Path, file: &str) -> Result<Shape> {
    match lstat_kind(target) {
        Ok(EntryKind::Missing) => Ok(Shape::Absent),
        Ok(EntryKind::Symlink) => Ok(Shape::Link),
        Ok(EntryKind::Dir) => Ok(Shape::Directory),
        Ok(EntryKind::File | EntryKind::Other) => Ok(Shape::Other),
        Err(error) => Err(unreadable(
            file,
            target,
            io_message(&error, "lstat", target),
        )),
    }
}

/// Every file under `dir`, relative, `/`-separated and sorted.
///
/// Directories are not listed on their own: an empty directory is not a difference worth a row, and
/// every difference that matters involves a file. `label` is how the tree is named in errors.
///
/// # Errors
///
/// Exit 2 when it cannot be listed.
fn file_list(dir: &Path, label: &str) -> Result<Vec<String>> {
    fn walk(
        current: &Path,
        relative: &str,
        found: &mut Vec<String>,
    ) -> std::result::Result<(), (io::Error, PathBuf, &'static str)> {
        let names = read_dir_names(current).map_err(|e| (e, current.to_path_buf(), "scandir"))?;

        for name in names {
            let within = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let entry = current.join(&name);
            let kind = lstat_kind(&entry).map_err(|e| (e, entry.clone(), "lstat"))?;

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
        .map_err(|(error, at, syscall)| unreadable(label, dir, io_message(&error, syscall, &at)))?;
    found.sort_by(|a, b| js_cmp(a, b));
    Ok(found)
}

/// Whether two files hold the same bytes.
///
/// A file that cannot be read counts as differing rather than as an error: "this is no longer what
/// the catalog ships" is true either way, and it is the answer someone can act on.
fn same_bytes(source: &Path, target: &Path) -> bool {
    match (std::fs::read(source), std::fs::read(target)) {
        (Ok(expected), Ok(actual)) => expected == actual,
        _ => false,
    }
}

/// The first difference between a materialized directory's source and what is installed, or
/// `None` when the two agree.
///
/// Reports one difference, not all of them, and the first in sorted order rather than the first
/// found, so two identical projects report identically. A status row needs one concrete thing to
/// look at; a full diff belongs to a diff tool.
///
/// # Errors
///
/// Exit 2 when either tree cannot be listed.
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

/// Compares one installed symlink against the source it should name.
///
/// The link is read rather than followed, and reported as written, with `/` separators: a relative
/// link is what `apply` creates and what someone sees in `ls -l`, so that is what a detail line
/// should say. Reporting the resolved absolute path would put a machine-specific string into
/// `status --json`.
///
/// # Errors
///
/// Exit 2 when the link cannot be read.
fn link_verdict(path: &str, target: &Path, source: &Path) -> Result<Verdict> {
    let written = std::fs::read_link(target)
        .map_err(|error| unreadable(path, target, io_message(&error, "readlink", target)))?;
    let written = written.to_string_lossy();

    // Resolved against the link's own directory, so a relative link and an absolute one naming the
    // same directory compare equal. Deliberately not `canonicalize`: this checks where the link
    // points, not what symlinks above it resolve to.
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

/// The skills link: present, ambit's, and pointing where the plan says.
///
/// A directory here is the pre-shared-layout install, which `install` migrates by replacing it, so
/// it reads as modified rather than as something in the way.
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

/// Compares one planned directory, a skill's or a hook's shipped script, against the project.
///
/// Checks existence, then ownership, then contents: something ambit did not create is `unowned`
/// whatever it holds, since install would refuse it rather than compare it.
///
/// What is on disk decides how the comparison is made, not the plan's `mode`: a link is checked for
/// pointing at its source, a directory compared byte for byte. Both modes put the same bytes in
/// front of the harness, so a `--copy` install with intact copies reads as clean even though a
/// plain `install` would relink it. Mode divergence is reported by `doctor` instead.
///
/// One function handles both kinds; the [`PlannedCatalogDir`] argument type keeps a hook directory
/// from being handed to [`config_verdict`] and misread as a document.
///
/// # Errors
///
/// Exit 2 when the target cannot be inspected.
fn catalog_dir_verdict(artifact: &PlannedCatalogDir, owned: &IndexSet<String>) -> Result<Verdict> {
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

    Ok(match first_difference(artifact)? {
        None => Verdict::OK,
        Some(difference) => Verdict::new(ArtifactState::Modified, difference),
    })
}

/// Compares the managed keys of one co-owned config file against what is in it.
///
/// The first problem in plan order decides the row, matching how ownership enforcement refuses on
/// the first conflict, so which key is reported depends on the bundle, not the file's layout. Stale
/// keys come last since they describe the previous install, not the current one.
///
/// Drift is decided by asking the driver whether one entry is already written as install would
/// write it, not by comparing parsed values: two of the three formats cannot be parsed without
/// losing what a person wrote.
///
/// In an array section the digest is the key, so an edited hook entry is not a changed value but an
/// absent key, and the row reads `missing`. This matters because install would append ambit's entry
/// beside the edited one, so the row must appear before that run, not as a second hook afterwards.
/// An edited declaration reads the same way, and prunes on the next install.
///
/// `stale` is the keys prior state claims here that the plan no longer writes, sorted.
///
/// # Errors
///
/// Exit 2 if the file exists but cannot be parsed.
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

/// The plan indexed by path, so a file two adapters write into is compared once.
fn planned_by_path(plan: &[PlannedArtifact]) -> IndexMap<&str, Vec<&PlannedArtifact>> {
    let mut by_path: IndexMap<&str, Vec<&PlannedArtifact>> = IndexMap::new();

    for artifact in plan {
        by_path.entry(artifact.path()).or_default().push(artifact);
    }

    by_path
}

/// Every managed key the plan writes into one config file, across every artifact naming it.
fn planned_keys<'a>(artifacts: &[&'a PlannedHarnessConfig]) -> IndexSet<&'a str> {
    artifacts
        .iter()
        .flat_map(|artifact| artifact.managed_keys.iter().map(String::as_str))
        .collect()
}

/// Compares a plan and the previous install's state against what is on disk.
///
/// Sorted by path so two identical projects report identically and a reader can find a row: the
/// order the adapters planned in is an implementation detail, but a path is what they came to look
/// up.
///
/// Needs no project root of its own: a planned artifact carries its absolute target, and a stale
/// one is only reported here, not removed.
///
/// # Errors
///
/// Exit 2 for a target that cannot be inspected or a config file that cannot be parsed. Neither is
/// drift; both mean the comparison could not be made.
fn compare_artifacts(plan: &[PlannedArtifact], prior: &State) -> Result<Vec<StatusArtifact>> {
    let owned = owned_paths(prior);
    let groups = planned_by_path(plan);
    let mut rows = Vec::new();

    for (&file, group) in &groups {
        // A group is built from the plan, so it always has a member and every member shares a
        // kind. Two artifacts of different kinds at one path would be an adapter bug, not a
        // project's problem.
        let Some(&first) = group.first() else {
            continue;
        };

        match first {
            PlannedArtifact::SkillDir(dir) | PlannedArtifact::HookDir(dir) => {
                rows.push(catalog_dir_verdict(dir, &owned)?.at(file, first.kind()));
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

    // What state still claims and the plan no longer writes: install would prune it.
    // One row per path, since two adapters writing into one config file record one entry each.
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

/// Compares an already-planned install against the project: the comparison without the
/// resolution.
///
/// Used by `doctor`, which needs both this verdict and the rest of `plan_install`'s output and must
/// not resolve the project twice to get them. Taking the plan as an argument keeps the two commands
/// from disagreeing: there is one comparison, and `status` is it.
///
/// `plan` is every adapter's planned artifacts, flattened; `prior` is what the last install
/// recorded owning.
///
/// # Errors
///
/// Exit 2 for a target that cannot be inspected or a config file that cannot be parsed.
pub fn status_of_plan(plan: &[PlannedArtifact], prior: &State) -> Result<ProjectStatus> {
    Ok(ProjectStatus {
        artifacts: compare_artifacts(plan, prior)?,
    })
}

/// Compares a project against what resolution now produces.
///
/// Plans through the adapters rather than reasoning about state alone, since the question is what
/// install would do: an adapter's plan is pure, so asking it costs nothing and the two commands
/// cannot disagree about where an artifact belongs.
///
/// # Errors
///
/// Exit 2 for a malformed config or catalog, an unknown harness, an unreadable state file, or a
/// target that cannot be inspected; exit 3 for a resolution error; exit 4 if a fetch fails, or under
/// `--offline` when the cache cannot answer. Drift itself is never an error.
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

    // No environment involved on either side beyond the scope install decided from the same root:
    // install writes a reference rather than a value, so a plan reads the same on every machine and
    // a set variable can never read as drift.
    let project = ProjectPaths {
        root: project_dir.to_path_buf(),
        scope: Some(install_scope(project_dir, env)),
        mode: None,
    };
    // Through `plan_for`, so status sees the artifacts install would write: one entry per shared
    // skills target, not one per harness reading it.
    let plan: Vec<PlannedArtifact> = plan_for(&adapters, &bundle, &project)
        .into_iter()
        .flat_map(|planned| planned.plan)
        .collect();

    status_of_plan(&plan, &read_state(project_dir)?)
}
