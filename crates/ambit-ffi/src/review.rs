//! Reviewing staged changes, applying exactly what was reviewed, and checking and reviewing
//! catalog updates.
//!
//! A [`Review`] holds the core's reviewed plan; the app shows its [`ReviewSummary`] and hands the
//! same object back to [`SetupSession::apply`]. The session remembers the commits of the last
//! applied review, so [`SetupSession::retry_install`] reproduces them after a partial failure.

use std::sync::Arc;

use ambit_core::errors::AmbitError;
use ambit_core::harness::adapter::{HookSkipReason, SkippedHook};
use ambit_core::model::config::find_config_file;
use ambit_core::project::bundle_diff::{BundleChange, BundleChangeKind, all_changes};
use ambit_core::project::operation_lock::is_operation_in_progress;
use ambit_core::project::ownership::OWNERSHIP_CONFLICT;
use ambit_core::project::prune::PrunedArtifact;
use ambit_core::project::review::{
    ApplyOutcome as CoreOutcome, Blocker, ReviewInput, STALE_REVIEW, SetupReview, apply_review,
    retry_install, review_setup,
};
use ambit_core::project::update::{CatalogFreshness, UpdateOptions, check_outdated};
use ambit_core::util::control::Control;
use indexmap::IndexMap;

use crate::control::{CancelToken, ProgressListener, control_of};
use crate::edit::ConfigChanges;
use crate::engine::{Engine, SetupSession};
use crate::errors::{EngineError, guard};
use crate::git::git_env;
use crate::records::{ItemKind, SelectionEntry, Stage};
use crate::status::{HealthFinding, InstallState, ManagedKind};

/// A reviewed set of changes. Apply it with [`SetupSession::apply`], or drop it to discard.
#[derive(uniffi::Object)]
pub struct Review(SetupReview);

#[uniffi::export]
impl Review {
    /// What applying would do, and what blocks it.
    pub fn summary(&self) -> ReviewSummary {
        summary_of(&self.0)
    }

    /// Whether nothing blocks applying.
    pub fn can_apply(&self) -> bool {
        self.0.can_apply()
    }

    /// The config text applying saves.
    pub fn config_text(&self) -> String {
        self.0.config_text.clone()
    }
}

/// A catalog whose installed commit applying moves.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct RevisionChange {
    pub catalog: String,
    /// What is installed now, absent when nothing is recorded.
    pub before: Option<String>,
    /// What applying installs, absent for a removed catalog.
    pub after: Option<String>,
}

/// How a capability changes.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Changed,
    Removed,
}

/// A capability added, removed or changed. `detail` names the pack or dependency that brings it,
/// or what changed.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CapabilityChange {
    pub kind: ItemKind,
    pub name: String,
    pub change: ChangeKind,
    pub detail: String,
}

/// What applying does to a managed path.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathChange {
    Create,
    Rewrite,
    Remove,
}

/// One managed path applying writes or removes. `entries` names the config entries removed from
/// a shared config file.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ManagedPathChange {
    pub path: String,
    pub kind: ManagedKind,
    pub change: PathChange,
    pub entries: Vec<String>,
}

/// Why an agent tool will not receive a hook.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The tool has no hook mechanism.
    NoHooks,
    /// The tool has hooks, but none for this event.
    UnsupportedEvent,
}

/// A capability and agent tool combination that will be skipped.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SkippedCombination {
    pub tool: String,
    pub hook: String,
    pub event: String,
    pub reason: SkipReason,
}

/// Something that prevents applying.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum ReviewBlocker {
    /// The configuration is invalid, or a catalog cannot be read.
    Config {
        message: String,
        detail: Vec<String>,
    },
    /// A selection matching nothing.
    Unmatched {
        entry: SelectionEntry,
        message: String,
        detail: Vec<String>,
    },
    /// The selection cannot be resolved: a cycle, or one name from two catalogs.
    Resolution {
        message: String,
        detail: Vec<String>,
    },
    /// An existing path or config entry ambit does not own. `detail` ends with the corrective
    /// action; ambit never takes it over.
    Ownership {
        path: String,
        key: Option<String>,
        message: String,
        detail: Vec<String>,
    },
}

/// What applying a review would do.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ReviewSummary {
    pub can_apply: bool,
    /// Whether applying saves the configuration file.
    pub config_changed: bool,
    pub config: ConfigChanges,
    pub revisions: Vec<RevisionChange>,
    pub capabilities: Vec<CapabilityChange>,
    pub paths: Vec<ManagedPathChange>,
    pub skipped: Vec<SkippedCombination>,
    pub limitations: Vec<HealthFinding>,
    pub lock_changed: bool,
    pub blockers: Vec<ReviewBlocker>,
}

fn capability(change: &BundleChange) -> CapabilityChange {
    CapabilityChange {
        kind: change.kind.into(),
        name: change.name.clone(),
        change: match change.change {
            BundleChangeKind::Added => ChangeKind::Added,
            BundleChangeKind::Changed => ChangeKind::Changed,
            BundleChangeKind::Removed => ChangeKind::Removed,
        },
        detail: change.detail.clone(),
    }
}

fn removal(pruned: &PrunedArtifact) -> ManagedPathChange {
    ManagedPathChange {
        path: pruned.path.clone(),
        kind: pruned.kind.into(),
        change: PathChange::Remove,
        entries: pruned.managed_keys.clone().unwrap_or_default(),
    }
}

fn skipped(hook: &SkippedHook) -> SkippedCombination {
    SkippedCombination {
        tool: hook.harness.clone(),
        hook: hook.hook.clone(),
        event: hook.event.to_string(),
        reason: match hook.reason {
            HookSkipReason::NoMechanism => SkipReason::NoHooks,
            HookSkipReason::NoEvent => SkipReason::UnsupportedEvent,
        },
    }
}

fn blocker(blocker: &Blocker) -> ReviewBlocker {
    match blocker {
        Blocker::Config(error) => ReviewBlocker::Config {
            message: error.message.clone(),
            detail: error.detail.clone(),
        },
        Blocker::Unmatched { entry, error } => ReviewBlocker::Unmatched {
            entry: SelectionEntry::from(entry),
            message: error.message.clone(),
            detail: error.detail.clone(),
        },
        Blocker::Resolution(error) => ReviewBlocker::Resolution {
            message: error.message.clone(),
            detail: error.detail.clone(),
        },
        Blocker::Ownership(conflict) => ReviewBlocker::Ownership {
            path: conflict.path.clone(),
            key: conflict.key.clone(),
            message: conflict.message.clone(),
            detail: conflict.detail.clone(),
        },
    }
}

fn summary_of(review: &SetupReview) -> ReviewSummary {
    let summary = &review.summary;
    let empty = ambit_core::resolution::resolve::Bundle::default();
    let bundle = review
        .planned
        .as_ref()
        .map_or(&empty, |planned| &planned.bundle);
    let mut paths: Vec<ManagedPathChange> = summary
        .writes
        .iter()
        .map(|write| ManagedPathChange {
            path: write.path.clone(),
            kind: write.kind.into(),
            change: if InstallState::from(write.state) == InstallState::Missing {
                PathChange::Create
            } else {
                PathChange::Rewrite
            },
            entries: Vec::new(),
        })
        .collect();

    paths.extend(summary.removals.iter().map(removal));

    ReviewSummary {
        can_apply: review.can_apply(),
        config_changed: summary.config_changed,
        config: ConfigChanges::from(summary.config.clone()),
        revisions: summary
            .revisions
            .iter()
            .map(|revision| RevisionChange {
                catalog: revision.name.clone(),
                before: revision.before.clone(),
                after: revision.after.clone(),
            })
            .collect(),
        capabilities: all_changes(&summary.diff).iter().map(capability).collect(),
        paths,
        skipped: summary.skipped.iter().map(skipped).collect(),
        limitations: summary
            .limitations
            .iter()
            .map(|finding| HealthFinding::of(finding, bundle))
            .collect(),
        lock_changed: summary.lock_changed,
        blockers: summary.blockers.iter().map(blocker).collect(),
    }
}

/// What an apply or a retry did.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// Everything reviewed is installed.
    Installed {
        tools: Vec<String>,
        /// Managed paths now owned.
        installed: u32,
        /// Managed paths and config entries removed.
        removed: u32,
    },
    /// Writing started and failed: "Changes not fully installed". The saved configuration stays;
    /// files already written stay and nothing is rolled back. `stage` and `subject` identify the
    /// failed operation; offer [`SetupSession::retry_install`].
    NotFullyInstalled {
        /// Whether this run saved the configuration.
        saved: bool,
        stage: Stage,
        /// The path being written, or empty for a step over the whole setup.
        subject: String,
        message: String,
        detail: Vec<String>,
    },
}

fn count(length: usize) -> u32 {
    u32::try_from(length).unwrap_or(u32::MAX)
}

fn outcome(engine: &Engine, outcome: CoreOutcome) -> ApplyOutcome {
    match outcome {
        CoreOutcome::Installed(result) => ApplyOutcome::Installed {
            tools: result.harnesses.clone(),
            installed: count(result.artifacts.len()),
            removed: count(result.pruned.len()),
        },
        CoreOutcome::NotFullyInstalled { saved, failure } => {
            let error = engine.error(&failure.error);

            ApplyOutcome::NotFullyInstalled {
                saved,
                stage: failure.stage.into(),
                subject: failure.subject,
                message: error.message().to_owned(),
                detail: match error {
                    EngineError::Config { detail, .. }
                    | EngineError::Resolution { detail, .. }
                    | EngineError::Network { detail, .. }
                    | EngineError::Io { detail, .. }
                    | EngineError::Internal { detail, .. } => detail,
                    _ => Vec::new(),
                },
            }
        }
    }
}

/// Converts an apply's error, recognizing the refusals the core marks by message.
fn apply_error(engine: &Engine, error: &AmbitError) -> EngineError {
    let token = engine.github_token();
    let secrets: Vec<&str> = token.as_deref().into_iter().collect();

    if error.message == STALE_REVIEW {
        return EngineError::stale_review(error, &secrets);
    }

    if error.message == OWNERSHIP_CONFLICT {
        return EngineError::ownership_conflict(error, None, &secrets);
    }

    if is_operation_in_progress(error) {
        return EngineError::busy(error, &secrets);
    }

    engine.error(error)
}

/// Where a catalog stands against its remote.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogFreshnessState {
    /// A newer commit is available.
    Outdated,
    /// The installed commit is the newest.
    Current,
    /// The catalog names a commit, so it cannot move.
    Pinned,
    /// A local catalog: its files are used as they are.
    Local,
}

/// The result of checking one catalog for updates. Checking changes nothing installed.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CatalogUpdateCheck {
    pub catalog: String,
    pub freshness: CatalogFreshnessState,
    /// The commit installed now.
    pub commit: Option<String>,
    /// The commit the catalog's ref names on the remote: what reviewing the update installs.
    pub latest: Option<String>,
    /// What moving to `latest` would change.
    pub changes: Vec<CapabilityChange>,
}

/// One catalog's reviewed revision, for [`SetupSession::review_catalog_updates`].
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ReviewedRevision {
    pub catalog: String,
    pub commit: String,
}

/// The commits of the last review this session applied, for a retry.
#[derive(Default)]
struct ApplyState {
    commits: IndexMap<String, String>,
}

impl SetupSession {
    fn review_with(
        &self,
        input: &ReviewInput,
        control: &Control,
    ) -> Result<Arc<Review>, EngineError> {
        let engine = self.engine();

        review_setup(self.root_path(), &git_env(engine), input, control)
            .map(|review| Arc::new(Review(review)))
            .map_err(|error| engine.error(&error))
    }

    /// The saved config's filename, for reviews of the saved config.
    fn saved_file(&self) -> Result<String, EngineError> {
        find_config_file(self.root_path())
            .map(|found| found.file)
            .map_err(|error| self.engine().error(&error))
    }
}

/// Review, apply and catalog updates.
// UniFFI passes every argument by value, so a borrowed parameter is not an option here.
#[allow(clippy::needless_pass_by_value)]
#[uniffi::export]
impl SetupSession {
    /// Plans `draft_text` (or the saved config, when `None`) without writing anything and
    /// summarizes it. `file_name` is the config filename to save to: the existing one, or
    /// `ambit.yml` for a new setup. Problems with the draft are blockers in the summary.
    ///
    /// # Errors
    ///
    /// [`EngineError::Network`] when a catalog cannot be fetched, [`EngineError::Canceled`],
    /// [`EngineError::Config`] when the setup folder cannot be read.
    pub fn review(
        &self,
        draft_text: Option<String>,
        file_name: String,
        cancel: Option<Arc<CancelToken>>,
        progress: Option<Arc<dyn ProgressListener>>,
    ) -> Result<Arc<Review>, EngineError> {
        guard(|| {
            self.review_with(
                &ReviewInput {
                    draft_text,
                    file: file_name,
                    pins: IndexMap::new(),
                },
                &control_of(cancel.as_ref(), progress),
            )
        })
    }

    /// Checks one catalog for a newer revision without changing what is installed.
    ///
    /// # Errors
    ///
    /// [`EngineError::Network`] when the catalog cannot be reached: never reported as current.
    /// [`EngineError::Config`] for an unknown catalog or an invalid config.
    pub fn check_catalog_update(&self, catalog: String) -> Result<CatalogUpdateCheck, EngineError> {
        guard(|| {
            let engine = self.engine();
            let plan = check_outdated(
                self.root_path(),
                &git_env(engine),
                &UpdateOptions {
                    catalogs: vec![catalog.clone()],
                },
            )
            .map_err(|error| engine.error(&error))?;
            let pin = plan
                .catalogs
                .iter()
                .find(|pin| pin.name == catalog)
                .ok_or_else(|| EngineError::Internal {
                    message: format!("no result for catalog \"{catalog}\""),
                    detail: Vec::new(),
                })?;

            Ok(CatalogUpdateCheck {
                catalog: pin.name.clone(),
                freshness: match pin.freshness {
                    CatalogFreshness::Outdated => CatalogFreshnessState::Outdated,
                    CatalogFreshness::Current => CatalogFreshnessState::Current,
                    CatalogFreshness::Pinned => CatalogFreshnessState::Pinned,
                    CatalogFreshness::Unversioned => CatalogFreshnessState::Local,
                },
                commit: pin.commit.clone(),
                latest: pin.latest.clone(),
                changes: all_changes(&plan.diff).iter().map(capability).collect(),
            })
        })
    }

    /// Reviews installing exactly the given commits for their catalogs, over the saved config.
    /// Applying the result never installs a newer commit than reviewed.
    ///
    /// # Errors
    ///
    /// As [`SetupSession::review`], plus [`EngineError::Config`] when there is no saved config.
    pub fn review_catalog_updates(
        &self,
        updates: Vec<ReviewedRevision>,
        cancel: Option<Arc<CancelToken>>,
        progress: Option<Arc<dyn ProgressListener>>,
    ) -> Result<Arc<Review>, EngineError> {
        guard(|| {
            let file = self.saved_file()?;

            self.review_with(
                &ReviewInput {
                    draft_text: None,
                    file,
                    pins: updates
                        .into_iter()
                        .map(|update| (update.catalog, update.commit))
                        .collect(),
                },
                &control_of(cancel.as_ref(), progress),
            )
        })
    }

    /// Applies `review`: checks it still holds, saves the config, installs. Cancellation is
    /// honoured until the config is saved.
    ///
    /// # Errors
    ///
    /// [`EngineError::StaleReview`] when the setup changed since the review (review again);
    /// [`EngineError::OwnershipConflict`] when a target became unowned; [`EngineError::Busy`]
    /// when another operation holds the setup; [`EngineError::Config`] for blockers or a failed
    /// save, in which case nothing was installed; [`EngineError::Canceled`].
    pub fn apply(
        &self,
        review: Arc<Review>,
        cancel: Option<Arc<CancelToken>>,
        progress: Option<Arc<dyn ProgressListener>>,
    ) -> Result<ApplyOutcome, EngineError> {
        guard(|| {
            let engine = self.engine();
            let control = control_of(cancel.as_ref(), progress);
            let applied = apply_review(self.root_path(), &git_env(engine), &review.0, &control)
                .map_err(|error| apply_error(engine, &error))?;

            self.with_state::<ApplyState, _>(|state| {
                state.commits.clone_from(&review.0.catalog_commits);
            });

            Ok(outcome(engine, applied))
        })
    }

    /// Installs the saved config again after "Changes not fully installed", reproducing the
    /// commits of the last review this session applied. Files a failed run left behind are not
    /// adopted: they come back as [`EngineError::OwnershipConflict`].
    ///
    /// # Errors
    ///
    /// As [`SetupSession::apply`], without stale reviews.
    pub fn retry_install(
        &self,
        cancel: Option<Arc<CancelToken>>,
        progress: Option<Arc<dyn ProgressListener>>,
    ) -> Result<ApplyOutcome, EngineError> {
        guard(|| {
            let engine = self.engine();
            let commits = self.with_state::<ApplyState, _>(|state| state.commits.clone());
            let control = control_of(cancel.as_ref(), progress);
            let retried = retry_install(self.root_path(), &git_env(engine), &commits, &control)
                .map_err(|error| apply_error(engine, &error))?;

            Ok(outcome(engine, retried))
        })
    }
}

#[cfg(test)]
mod tests;
