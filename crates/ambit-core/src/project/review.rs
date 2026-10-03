//! Reviewing a setup's changes before applying them, and applying exactly what was reviewed.
//!
//! The app stages edits as a draft config. [`review_setup`] plans that draft against the project
//! without writing anything and summarizes what applying it would do. [`apply_review`] then
//! installs that plan, or refuses with [`STALE_REVIEW`] when it can no longer be the same plan.
//!
//! "The same plan" is checked three ways before the first write, all under the setup lock:
//!
//! - The [`SetupFingerprint`] of the project files and `path:` catalogs must be unchanged.
//! - The draft is planned again offline, with every git catalog pinned to the commit the review
//!   resolved it to, so a branch that moved since cannot change what is installed. The new plan's
//!   signature (its lock text and every artifact's identity) must equal the reviewed one.
//! - Ownership is checked again in full, and any conflict refuses the apply. Nothing is adopted.
//!
//! Only then is the config saved, atomically and only if its bytes differ, and the plan executed.
//! A failure after the save is [`ApplyOutcome::NotFullyInstalled`]: the saved config stays, files
//! already written stay (unowned, since state is written last), and [`retry_install`] reports
//! them as ownership conflicts rather than taking them over.
//!
//! Reviews resolve an unpinned git catalog with [`RefreshMode::Probe`], which asks the remote
//! without moving the shared clone's refs; the commit it reports is what gets installed. The CLI's
//! `ambit install` advances the clone instead (see `catalog_plan` in `install.rs`).

use std::path::Path;

use indexmap::IndexMap;
use serde_json::json;

use crate::errors::{AmbitError, ExitCode, Result, config_error};
use crate::harness::adapter::SkippedHook;
use crate::model::catalog::merge_catalogs;
use crate::model::config::{
    CONFIG_FILENAMES, ProjectConfig, existing_config_files, find_config_file, load_project_config,
    parse_project_config,
};
use crate::model::config_edit::{ConfigChanges, config_changes};
use crate::model::git::RefreshMode;
use crate::model::lock_file::{read_catalog_pins, read_lock_text};
use crate::model::pattern::PatternEntry;
use crate::model::state::ArtifactKind;
use crate::project::bundle_diff::{BundleDiff, diff_bundles};
use crate::project::config_file::save_config_atomic;
use crate::project::doctor::{DoctorFinding, harness_findings};
use crate::project::fingerprint::{SetupFingerprint, setup_fingerprint};
use crate::project::install::{
    InstallFailure, InstallOptions, InstallResult, PlanContext, PlannedInstall, execute_install,
    identity_of, load_plan_catalogs, plan_bundle, plan_install_from_config,
};
use crate::project::operation_lock::SetupLock;
use crate::project::ownership::{
    OwnershipConflict, OwnershipOptions, authorize_plan, conflicts_error, ownership_conflicts,
};
use crate::project::prune::{PrunedArtifact, plan_prune};
use crate::project::status::{ArtifactState, status_of_plan};
use crate::resolution::resolve::Bundle;
use crate::resolution::routes::{resolve_matched, unmatched_entries};
use crate::util::control::{Control, Progress, Stage};
use crate::util::env::Env;
use crate::util::fs::read_text;
use crate::util::hash::sha256_hex;
use crate::util::json;
use crate::util::path::join;

#[cfg(test)]
mod tests;

/// The summary of the error an apply returns when the setup changed since its review.
///
/// Exported so the app's bindings can tell this refusal apart from other config errors.
pub const STALE_REVIEW: &str = "the setup changed since these changes were reviewed";

/// The stale-review error, exit 2, with `what` naming the change.
pub fn stale_review(what: impl Into<String>) -> AmbitError {
    config_error(
        STALE_REVIEW,
        [
            what.into(),
            "review the changes again, so what is applied is what you saw".to_owned(),
        ],
    )
}

/// What to review.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewInput {
    /// The draft config text. `None` reviews the saved config, to reapply it or to apply a catalog
    /// update.
    pub draft_text: Option<String>,
    /// The config filename the draft is saved as: the existing file's name, or `ambit.yml` for a
    /// new setup.
    pub file: String,
    /// Commits to resolve catalogs to, by name, over the lock's: a reviewed catalog update's
    /// checked commits (`CatalogPin::latest`). Empty otherwise.
    pub pins: IndexMap<String, String>,
}

/// A catalog whose installed revision applying would change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRevision {
    pub name: String,
    /// The commit `ambit.lock` records for it, absent when it records none.
    pub before: Option<String>,
    /// The commit the review resolved it to, absent for a removed catalog.
    pub after: Option<String>,
}

/// A managed path applying would write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedPath {
    /// Project-relative, `/`-separated.
    pub path: String,
    pub kind: ArtifactKind,
    /// `missing` for a new path, `modified` for one that will be rewritten.
    pub state: ArtifactState,
}

/// Something that keeps a review from being applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blocker {
    /// The draft does not parse or validate, the config filename is wrong, or a catalog cannot be
    /// read.
    Config(AmbitError),
    /// A `requires` entry matching nothing.
    Unmatched {
        entry: PatternEntry,
        error: AmbitError,
    },
    /// The selection cannot be resolved for another reason: a cycle, a name from two catalogs.
    Resolution(AmbitError),
    /// A target ambit would overwrite and does not own.
    Ownership(OwnershipConflict),
}

impl Blocker {
    /// The summary line, for an error listing every blocker.
    pub fn message(&self) -> String {
        match self {
            Self::Config(error) | Self::Resolution(error) | Self::Unmatched { error, .. } => {
                error.message.clone()
            }
            Self::Ownership(conflict) => conflict.message.clone(),
        }
    }
}

/// What applying a review would do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReviewSummary {
    pub config: ConfigChanges,
    /// Whether the config text differs from the saved file, so applying saves it.
    pub config_changed: bool,
    /// Every catalog whose revision moves, in config order, then removed ones.
    pub revisions: Vec<CatalogRevision>,
    /// Capabilities added, removed and changed against what the saved config installs, with pack
    /// and dependency effects in each change's detail.
    pub diff: BundleDiff,
    /// Managed paths that will be written, sorted.
    pub writes: Vec<ManagedPath>,
    /// Managed paths and config entries that will be removed.
    pub removals: Vec<PrunedArtifact>,
    /// Hooks a configured agent tool cannot express, so will not receive.
    pub skipped: Vec<SkippedHook>,
    /// What a configured agent tool needs that ambit cannot write: `doctor`'s harness findings.
    pub limitations: Vec<DoctorFinding>,
    /// Whether `ambit.lock` changes.
    pub lock_changed: bool,
    pub blockers: Vec<Blocker>,
}

/// A reviewed draft, ready to apply. Holds everything [`apply_review`] checks against.
#[derive(Clone)]
pub struct SetupReview {
    /// The plan, absent when a blocker stopped planning.
    pub planned: Option<PlannedInstall>,
    /// The config text to save and install.
    pub config_text: String,
    /// The filename it is saved as.
    pub file: String,
    /// The saved config's text when the review was made, absent when there was none.
    pub base_text: Option<String>,
    /// The commit each git catalog resolved to, by name: what applying pins.
    pub catalog_commits: IndexMap<String, String>,
    pub fingerprint: SetupFingerprint,
    /// [`plan_signature`] of `planned`, empty without one.
    pub signature: String,
    pub summary: ReviewSummary,
}

impl SetupReview {
    /// Whether applying can proceed: there is a plan and nothing blocks it.
    pub fn can_apply(&self) -> bool {
        self.planned.is_some() && self.summary.blockers.is_empty()
    }
}

/// What an apply or a retry did once it got past its checks.
#[derive(Clone, Debug)]
pub enum ApplyOutcome {
    Installed(Box<InstallResult>),
    /// Writing started and failed. `saved` is whether this run saved the config; either way the
    /// config on disk is the one that was being installed.
    NotFullyInstalled {
        saved: bool,
        failure: InstallFailure,
    },
}

/// The identity of a plan: its lock text and every artifact's identity, hashed.
pub fn plan_signature(planned: &PlannedInstall) -> String {
    let identities: Vec<String> = planned.artifacts.iter().map(identity_of).collect();

    sha256_hex(json::stringify(&json!([planned.lock_text, identities])).as_bytes())
}

/// Sorts an error from planning into a blocker, or passes it on when it makes the review
/// meaningless: a network failure (retry), a cancel, a bug.
fn blocker_for(error: AmbitError) -> Result<Blocker> {
    match error.code {
        ExitCode::Config => Ok(Blocker::Config(error)),
        ExitCode::Resolution => Ok(Blocker::Resolution(error)),
        _ => Err(error),
    }
}

/// The saved config's name and text, and a blocker against saving the draft.
type SavedConfig = (Option<(String, String)>, Option<Blocker>);

/// Reads the saved config, refusing a draft saved under another name or into an ambiguous root.
fn saved_config(root: &Path, file: &str) -> Result<SavedConfig> {
    let existing = existing_config_files(root)?;

    match existing.as_slice() {
        [] => Ok((None, None)),
        [found] => {
            let path = join(root, found);
            let text = read_text(&path).map_err(|error| {
                config_error(
                    format!("cannot read {found}"),
                    [
                        crate::util::fs::io_message(&error, "open", &path),
                        format!("make {} readable", path.display()),
                    ],
                )
            })?;
            let blocker = (found != file).then(|| {
                Blocker::Config(config_error(
                    format!("this setup's config is {found}, not {file}"),
                    [format!("save the changes to {found}")],
                ))
            });

            Ok((Some((found.clone(), text)), blocker))
        }
        _ => Ok((
            None,
            Some(Blocker::Config(
                find_config_file(root).err().unwrap_or_else(|| {
                    config_error("more than one config file", Vec::<String>::new())
                }),
            )),
        )),
    }
}

/// What the saved config installs now, from the cache alone: the "before" of the review's diff.
/// Anything that keeps it from resolving reads as nothing installed.
fn installed_bundle(root: &Path, env: &Env, base: Option<&ProjectConfig>) -> Bundle {
    base.and_then(|config| {
        plan_install_from_config(
            root,
            config,
            env,
            InstallOptions {
                offline: true,
                ..InstallOptions::default()
            },
            &PlanContext::default(),
        )
        .ok()
    })
    .map(|planned| planned.bundle)
    .unwrap_or_default()
}

/// Each catalog whose commit differs between the lock and the plan.
fn revisions(
    root: &Path,
    base: Option<&ProjectConfig>,
    planned: &PlannedInstall,
    draft: &ProjectConfig,
) -> Result<Vec<CatalogRevision>> {
    let before = match base {
        Some(base) => read_catalog_pins(root, base)?,
        None => IndexMap::new(),
    };
    let mut names: Vec<&String> = draft.catalogs.iter().map(|catalog| &catalog.name).collect();

    let removed: Vec<&String> = before.keys().filter(|name| !names.contains(name)).collect();

    names.extend(removed);

    Ok(names
        .into_iter()
        .filter_map(|name| {
            let before = before.get(name).cloned();
            let after = planned.commits.get(name).cloned();

            (before != after).then(|| CatalogRevision {
                name: name.clone(),
                before,
                after,
            })
        })
        .collect())
}

/// Plans `config_text` as the app's draft, writing nothing, and summarizes what applying it would
/// do.
///
/// `root` is the setup root, absolute; `env` is the full environment. Problems with the draft
/// itself (it does not parse, an entry matches nothing, a target is unowned) are blockers in the
/// summary, not errors, so the app can show them all at once.
///
/// # Errors
///
/// Exit 2 when the root or the saved config cannot be read; exit 4 when a catalog cannot be
/// fetched; exit 130 once `control` is canceled.
pub fn review_setup(
    root: &Path,
    env: &Env,
    input: &ReviewInput,
    control: &Control,
) -> Result<SetupReview> {
    control.check()?;

    let mut blockers = Vec::new();

    if !CONFIG_FILENAMES.contains(&input.file.as_str()) {
        blockers.push(Blocker::Config(config_error(
            format!("\"{}\" is not a config filename", input.file),
            [format!("use one of: {}", CONFIG_FILENAMES.join(", "))],
        )));
    }

    let (saved, blocker) = saved_config(root, &input.file)?;

    blockers.extend(blocker);

    let base_text = saved.as_ref().map(|(_, text)| text.clone());
    let base = saved
        .as_ref()
        .and_then(|(file, text)| parse_project_config(text, file).ok());
    let config_text = input
        .draft_text
        .clone()
        .or_else(|| base_text.clone())
        .unwrap_or_default();

    if input.draft_text.is_none() && base_text.is_none() {
        blockers.push(Blocker::Config(config_error(
            "this setup has no saved configuration to apply",
            ["make a change to create one"],
        )));
    }

    let draft = match parse_project_config(&config_text, &input.file) {
        Ok(draft) => Some(draft),
        Err(error) => {
            blockers.push(Blocker::Config(error));
            None
        }
    };
    let fingerprint = setup_fingerprint(root, draft.as_ref())?;
    let mut review = SetupReview {
        planned: None,
        config_text,
        file: input.file.clone(),
        base_text,
        catalog_commits: IndexMap::new(),
        fingerprint,
        signature: String::new(),
        summary: ReviewSummary::default(),
    };

    review.summary.config_changed = review.base_text.as_deref() != Some(&review.config_text);

    let Some(draft) = draft.filter(|_| blockers.is_empty()) else {
        review.summary.blockers = blockers;
        return Ok(review);
    };

    review.summary.config = config_changes(base.as_ref(), &draft);

    let plan = PlanContext {
        refresh: Some(RefreshMode::Probe),
        pins: Some(input.pins.clone()),
        control: control.clone(),
        ..PlanContext::default()
    };
    let options = InstallOptions::default();
    let catalogs = match load_plan_catalogs(root, &draft, env, options, &plan) {
        Ok(catalogs) => catalogs,
        Err(error) => {
            blockers.push(blocker_for(error)?);
            review.summary.blockers = blockers;
            return Ok(review);
        }
    };
    let merged = merge_catalogs(&catalogs);

    blockers.extend(
        unmatched_entries(&draft, &merged)
            .into_iter()
            .map(|(entry, error)| Blocker::Unmatched { entry, error }),
    );

    control.report(&Progress {
        stage: Stage::Resolving,
        subject: String::new(),
        current: 0,
        total: 0,
    });

    // Without the unmatched entries, so the rest of the draft still shows what it would do.
    let planned = match resolve_matched(&draft, &merged)
        .and_then(|bundle| plan_bundle(root, &draft, env, options, &plan, &catalogs, bundle))
    {
        Ok(planned) => planned,
        Err(error) => {
            blockers.push(blocker_for(error)?);
            review.summary.blockers = blockers;
            return Ok(review);
        }
    };

    control.report(&Progress {
        stage: Stage::CheckingOwnership,
        subject: String::new(),
        current: 0,
        total: 0,
    });

    match ownership_conflicts(&planned.artifacts, &planned.prior) {
        Ok(conflicts) => blockers.extend(conflicts.into_iter().map(Blocker::Ownership)),
        Err(error) => blockers.push(blocker_for(error)?),
    }

    control.check()?;

    let status = status_of_plan(&planned.artifacts, &planned.prior)?;
    let before = installed_bundle(root, env, base.as_ref());

    review.summary = ReviewSummary {
        config: std::mem::take(&mut review.summary.config),
        config_changed: review.summary.config_changed,
        revisions: revisions(root, base.as_ref(), &planned, &draft)?,
        diff: diff_bundles(&before, &planned.bundle)?,
        writes: status
            .artifacts
            .iter()
            .filter(|row| matches!(row.state, ArtifactState::Missing | ArtifactState::Modified))
            .map(|row| ManagedPath {
                path: row.path.clone(),
                kind: row.kind,
                state: row.state,
            })
            .collect(),
        removals: plan_prune(&planned.artifacts, &planned.prior)?,
        skipped: planned.skipped.clone(),
        limitations: harness_findings(&planned.bundle, &planned.harnesses),
        lock_changed: read_lock_text(root)?.as_deref() != Some(planned.lock_text.as_str()),
        blockers,
    };
    review.catalog_commits.clone_from(&planned.commits);
    review.signature = plan_signature(&planned);
    review.planned = Some(planned);

    Ok(review)
}

/// Executes a plan that passed every check, mapping a write failure to
/// [`ApplyOutcome::NotFullyInstalled`].
fn execute(
    root: &Path,
    planned: &PlannedInstall,
    saved: bool,
    control: &Control,
) -> Result<ApplyOutcome> {
    // Adoption off: the conflicts were just checked empty, so this only adds the skills-directory
    // migration `authorize_plan` performs implicitly.
    let owner = authorize_plan(
        &planned.artifacts,
        &planned.prior,
        OwnershipOptions::default(),
    )?;

    Ok(match execute_install(root, planned, &owner, control) {
        Ok(result) => ApplyOutcome::Installed(Box::new(result)),
        Err(failure) => ApplyOutcome::NotFullyInstalled { saved, failure },
    })
}

/// Applies a review: re-checks it, saves the config, installs.
///
/// Every check happens under the setup lock and before the first write, so any error leaves the
/// project exactly as it was and the draft intact. Cancellation is honoured up to the save.
///
/// # Errors
///
/// Exit 2: a review with blockers; [`STALE_REVIEW`] when the project files, a `path:` catalog,
/// or the re-planned result differ from the review; [`OWNERSHIP_CONFLICT`](crate::project::ownership::OWNERSHIP_CONFLICT)
/// when a target became unowned; a failed save. Exit 4 when a reviewed commit has left the cache;
/// exit 130 when canceled before the save.
pub fn apply_review(
    root: &Path,
    env: &Env,
    review: &SetupReview,
    control: &Control,
) -> Result<ApplyOutcome> {
    if !review.can_apply() {
        return Err(config_error(
            "these changes cannot be applied",
            review.summary.blockers.iter().map(Blocker::message),
        ));
    }

    let _lock = SetupLock::acquire(root)?;

    control.check()?;
    control.report(&Progress {
        stage: Stage::CheckingOwnership,
        subject: String::new(),
        current: 0,
        total: 0,
    });

    let config = parse_project_config(&review.config_text, &review.file)?;
    let now = setup_fingerprint(root, Some(&config))?;

    if review.fingerprint.inputs_changed(&now) {
        return Err(stale_review(
            "the configuration, ambit.lock, .ambit/state.json or a managed .gitignore changed",
        ));
    }

    if let Some(name) = review.fingerprint.changed_catalogs(&now).first() {
        return Err(stale_review(format!(
            "the files of local catalog \"{name}\" changed"
        )));
    }

    let planned = plan_install_from_config(
        root,
        &config,
        env,
        InstallOptions {
            offline: true,
            ..InstallOptions::default()
        },
        &PlanContext {
            pins: Some(review.catalog_commits.clone()),
            control: control.clone(),
            ..PlanContext::default()
        },
    )?;

    if plan_signature(&planned) != review.signature {
        return Err(stale_review(
            "planning the reviewed configuration again gives a different result",
        ));
    }

    let conflicts = ownership_conflicts(&planned.artifacts, &planned.prior)?;

    if !conflicts.is_empty() {
        return Err(conflicts_error(&conflicts));
    }

    control.check()?;

    let saved = review.base_text.as_deref() != Some(review.config_text.as_str());

    if saved {
        control.report(&Progress {
            stage: Stage::SavingConfig,
            subject: review.file.clone(),
            current: 0,
            total: 0,
        });
        save_config_atomic(
            root,
            &review.file,
            review.base_text.as_deref(),
            &review.config_text,
        )?;
    }

    execute(root, &planned, saved, control)
}

/// Installs the saved config again after an apply that did not finish.
///
/// `pins` are the commits to reproduce, by catalog name: the failed review's
/// [`SetupReview::catalog_commits`], since the failed install never wrote them into the lock.
/// Empty pins resolve as a review does. Leftovers of the failed run are unowned, so they come back
/// as ownership conflicts, which this refuses rather than adopts.
///
/// # Errors
///
/// Exit 2 for a missing or invalid config, or
/// [`OWNERSHIP_CONFLICT`](crate::project::ownership::OWNERSHIP_CONFLICT) naming every conflict;
/// exit 3 for a resolution error; exit 4 for a fetch; exit 130 when canceled before writing.
pub fn retry_install(
    root: &Path,
    env: &Env,
    pins: &IndexMap<String, String>,
    control: &Control,
) -> Result<ApplyOutcome> {
    let _lock = SetupLock::acquire(root)?;
    let config = load_project_config(root)?;
    let planned = plan_install_from_config(
        root,
        &config,
        env,
        InstallOptions::default(),
        &PlanContext {
            refresh: Some(RefreshMode::Probe),
            pins: Some(pins.clone()),
            control: control.clone(),
            ..PlanContext::default()
        },
    )?;
    let conflicts = ownership_conflicts(&planned.artifacts, &planned.prior)?;

    if !conflicts.is_empty() {
        return Err(conflicts_error(&conflicts));
    }

    control.check()?;
    execute(root, &planned, false, control)
}
