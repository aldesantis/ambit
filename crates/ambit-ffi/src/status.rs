//! Installation status and health of a setup, read from disk and the catalog cache only.
//!
//! Neither export fetches: a catalog missing from the cache is an error, never an implicit
//! update. Health inspects the environment the app was given, which can differ from the one an
//! agent tool runs in; [`HealthReport::environment_note`] says so in words the app can show.

use ambit_core::model::state::ArtifactKind;
use ambit_core::project::doctor::{
    CheckStatus, DoctorCheck, DoctorFinding, DoctorSeverity, diagnose_planned,
};
use ambit_core::project::install::{InstallOptions, PlanContext, PlannedInstall, plan_install};
use ambit_core::project::status::{
    self as core_status, ArtifactState, ProjectStatus, StatusArtifact, item_statuses,
    status_of_plan,
};
use ambit_core::resolution::resolve::{Bundle, BundleItem};
use ambit_core::resolution::routes::bundle_catalog;

use crate::engine::SetupSession;
use crate::errors::{EngineError, guard};
use crate::git::git_env;
use crate::records::ItemRef;

/// `item` with the catalog `bundle` selected it from; empty when the bundle does not hold it.
pub fn item_ref(item: &BundleItem, bundle: &Bundle) -> ItemRef {
    ItemRef {
        kind: item.kind.into(),
        catalog: bundle_catalog(bundle, item).unwrap_or_default().to_owned(),
        name: item.name.clone(),
    }
}

/// What comparing an installed artifact against the plan concluded.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstallState {
    /// Installed exactly as an install would write it.
    Ok,
    /// Selected and not installed.
    Missing,
    /// Installed, and changed since.
    Modified,
    /// Installed by ambit and no longer selected; the next apply removes it.
    Stale,
    /// Something ambit did not create is in the way.
    Unowned,
}

impl From<ArtifactState> for InstallState {
    fn from(state: ArtifactState) -> Self {
        match state {
            ArtifactState::Ok => Self::Ok,
            ArtifactState::Missing => Self::Missing,
            ArtifactState::Modified => Self::Modified,
            ArtifactState::Stale => Self::Stale,
            ArtifactState::Unowned => Self::Unowned,
        }
    }
}

/// What kind of thing a managed path is.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManagedKind {
    SkillDirectory,
    HookDirectory,
    SkillsLink,
    /// Entries ambit owns inside an agent tool's own config file.
    ToolConfig,
}

impl From<ArtifactKind> for ManagedKind {
    fn from(kind: ArtifactKind) -> Self {
        match kind {
            ArtifactKind::SkillDir => Self::SkillDirectory,
            ArtifactKind::HookDir => Self::HookDirectory,
            ArtifactKind::SkillsLink => Self::SkillsLink,
            ArtifactKind::HarnessConfig => Self::ToolConfig,
        }
    }
}

/// One managed path and its state.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ArtifactStatus {
    /// Relative to the setup root, `/`-separated.
    pub path: String,
    pub kind: ManagedKind,
    pub state: InstallState,
    /// What differs, empty when `ok`.
    pub detail: String,
}

impl From<&StatusArtifact> for ArtifactStatus {
    fn from(artifact: &StatusArtifact) -> Self {
        Self {
            path: artifact.path.clone(),
            kind: artifact.kind.into(),
            state: artifact.state.into(),
            detail: artifact.detail.clone(),
        }
    }
}

/// One selected capability's installation.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ItemStatus {
    pub item: ItemRef,
    /// The worst state among `artifacts`.
    pub state: InstallState,
    /// Its directories and its entries in agent tool configs, deduplicated across tools.
    pub artifacts: Vec<ArtifactStatus>,
}

/// A setup's installation: per capability, and per managed path.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SetupStatus {
    /// Every selected skill, MCP server and hook that installs something, in report order. Packs
    /// install nothing and are absent.
    pub items: Vec<ItemStatus>,
    /// Every managed path, including stale ones no item selects any more, sorted.
    pub artifacts: Vec<ArtifactStatus>,
}

/// Which health check produced a finding.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthCheck {
    /// A declared prerequisite, or an environment variable a config references, is missing.
    Prerequisites,
    /// `ambit.lock` is missing or out of date.
    Lock,
    /// Something ambit installs is in the way of something ambit did not create.
    Ownership,
    /// An installed file changed, or is missing.
    Drift,
    /// A skill is copied where it would be linked, or the reverse.
    Mode,
    /// An agent tool cannot use something as installed.
    ToolLimitation,
}

impl From<DoctorCheck> for HealthCheck {
    fn from(check: DoctorCheck) -> Self {
        match check {
            DoctorCheck::Expects => Self::Prerequisites,
            DoctorCheck::Lock => Self::Lock,
            DoctorCheck::Ownership => Self::Ownership,
            DoctorCheck::Drift => Self::Drift,
            DoctorCheck::Mode => Self::Mode,
            DoctorCheck::Harness => Self::ToolLimitation,
        }
    }
}

/// How much a finding matters.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The setup will not do what it says.
    Problem,
    /// Worth knowing; the setup still works.
    Warning,
}

/// One health finding: what, about which capability and tool, and the next step.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct HealthFinding {
    pub check: HealthCheck,
    pub severity: Severity,
    /// The plain summary.
    pub message: String,
    /// The explanation, ending in one concrete next step.
    pub detail: Vec<String>,
    /// The capabilities it concerns. Empty when it concerns the setup as a whole.
    pub subjects: Vec<ItemRef>,
    /// The agent tool it concerns, when one.
    pub harness: Option<String>,
}

impl HealthFinding {
    /// `finding`, with its subjects' catalogs read from `bundle`.
    pub fn of(finding: &DoctorFinding, bundle: &Bundle) -> Self {
        Self {
            check: finding.check.into(),
            severity: match finding.severity {
                DoctorSeverity::Fail => Severity::Problem,
                DoctorSeverity::Warn => Severity::Warning,
            },
            message: finding.message.clone(),
            detail: finding.detail.clone(),
            subjects: finding
                .subjects
                .iter()
                .map(|item| item_ref(item, bundle))
                .collect(),
            harness: finding.harness.clone(),
        }
    }
}

/// One check's verdict.
#[derive(uniffi::Record, Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckOutcome {
    pub check: HealthCheck,
    /// Absent when the check found nothing; otherwise its worst finding's severity.
    pub worst: Option<Severity>,
}

/// What a health check found.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct HealthReport {
    /// Every check, in the order they run, so a clean report still says what was looked at.
    pub checks: Vec<CheckOutcome>,
    pub findings: Vec<HealthFinding>,
    pub items: Vec<ItemStatus>,
    /// Installed exactly as planned, and still waiting on a prerequisite: "Installed; setup
    /// required".
    pub setup_required: Vec<ItemRef>,
    /// Shown beside prerequisite findings: what this check can and cannot see.
    pub environment_note: String,
}

/// What [`HealthReport::environment_note`] says.
const ENVIRONMENT_NOTE: &str = "Ambit checks the environment it was started with. An agent tool can run with different environment variables, so a prerequisite reported here can be set there, and the reverse.";

/// The setup planned from the cache, without fetching or moving any pin.
fn plan_offline(session: &SetupSession) -> Result<PlannedInstall, EngineError> {
    let engine = session.engine();

    plan_install(
        session.root_path(),
        &git_env(engine),
        InstallOptions {
            offline: true,
            ..InstallOptions::default()
        },
        &PlanContext::default(),
    )
    .map_err(|error| engine.error(&error))
}

fn items_of(
    planned: &PlannedInstall,
    status: &ProjectStatus,
) -> Result<Vec<ItemStatus>, ambit_core::errors::AmbitError> {
    Ok(item_statuses(planned, status)?
        .iter()
        .map(|item: &core_status::ItemStatus| ItemStatus {
            item: ItemRef {
                kind: item.item.kind.into(),
                catalog: item.catalog.clone(),
                name: item.item.name.clone(),
            },
            state: item.state.into(),
            artifacts: item.artifacts.iter().map(ArtifactStatus::from).collect(),
        })
        .collect())
}

/// Reads a setup's status and health.
#[uniffi::export]
impl SetupSession {
    /// What is selected, what is installed, and what drifted, from the cache alone.
    ///
    /// # Errors
    ///
    /// [`EngineError::Config`] for a missing or invalid config, [`EngineError::Resolution`] for a
    /// selection that does not resolve, [`EngineError::Network`] (`notCached`) when a catalog is
    /// not in the cache.
    pub fn status(&self) -> Result<SetupStatus, EngineError> {
        guard(|| {
            let engine = self.engine();
            let planned = plan_offline(self)?;
            let status = status_of_plan(&planned.artifacts, &planned.prior)
                .map_err(|error| engine.error(&error))?;

            Ok(SetupStatus {
                items: items_of(&planned, &status).map_err(|error| engine.error(&error))?,
                artifacts: status.artifacts.iter().map(ArtifactStatus::from).collect(),
            })
        })
    }

    /// Every health check, the per-capability status, and which installed capabilities still need
    /// setup. Reads only; never collects secrets.
    ///
    /// # Errors
    ///
    /// As [`SetupSession::status`].
    pub fn health(&self) -> Result<HealthReport, EngineError> {
        guard(|| {
            let engine = self.engine();
            let planned = plan_offline(self)?;
            let convert = |error: ambit_core::errors::AmbitError| engine.error(&error);
            let report =
                diagnose_planned(self.root_path(), engine.env(), &planned).map_err(convert)?;
            let status = status_of_plan(&planned.artifacts, &planned.prior).map_err(convert)?;
            let items = items_of(&planned, &status).map_err(convert)?;
            let findings: Vec<HealthFinding> = report
                .findings
                .iter()
                .map(|finding| HealthFinding::of(finding, &planned.bundle))
                .collect();
            let setup_required = items
                .iter()
                .filter(|item| item.state == InstallState::Ok)
                .filter(|item| {
                    findings.iter().any(|finding| {
                        finding.check == HealthCheck::Prerequisites
                            && finding.subjects.contains(&item.item)
                    })
                })
                .map(|item| item.item.clone())
                .collect();

            Ok(HealthReport {
                checks: report
                    .checks
                    .iter()
                    .map(|result| CheckOutcome {
                        check: result.check.into(),
                        worst: match result.status {
                            CheckStatus::Ok => None,
                            CheckStatus::Warn => Some(Severity::Warning),
                            CheckStatus::Fail => Some(Severity::Problem),
                        },
                    })
                    .collect(),
                findings,
                items,
                setup_required,
                environment_note: ENVIRONMENT_NOTE.to_owned(),
            })
        })
    }
}
