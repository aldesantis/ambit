//! `ambit doctor`: preconditions, the lock, ownership, drift, mode, and harness limitations.

use std::path::Path;

use crate::errors::Result;
use crate::util::env::Env;
use crate::util::string_enum;

string_enum! {
    /// The checks, in the order they run and are reported: the world around the project, then the
    /// record of the last install, then what that record and the project disagree about, then the
    /// two that are merely worth knowing (materialization mode, harness limitations).
    ///
    /// `Expects` covers every kind of precondition an entity can declare, not one check per kind,
    /// since they share a verdict and a fix.
    pub enum DoctorCheck {
        Expects => "expects",
        Lock => "lock",
        Ownership => "ownership",
        Drift => "drift",
        Mode => "mode",
        Harness => "harness",
    }
}

/// Every check, in the order they run and are reported.
pub const DOCTOR_CHECKS: &[DoctorCheck] = DoctorCheck::ALL;

string_enum! {
    /// How much a finding matters.
    ///
    /// - `Fail`: the project will not do what it says it does. Exit 6.
    /// - `Warn`: worth knowing, and a legitimate way to run: reported, but never an exit code.
    pub enum DoctorSeverity {
        Fail => "fail",
        Warn => "warn",
    }
}

/// Every severity, in declaration order.
pub const DOCTOR_SEVERITIES: &[DoctorSeverity] = DoctorSeverity::ALL;

/// One finding, shaped like an error, since that is what it would have been.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctorFinding {
    pub check: DoctorCheck,
    pub severity: DoctorSeverity,
    /// The summary: the offending identifier, and the file it is in.
    pub message: String,
    /// The remaining lines, ending in one concrete next step.
    pub detail: Vec<String>,
}

string_enum! {
    /// What one check concluded: its worst finding, or `Ok` when it found none.
    pub enum CheckStatus {
        Fail => "fail",
        Ok => "ok",
        Warn => "warn",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckResult {
    pub check: DoctorCheck,
    pub status: CheckStatus,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DoctorReport {
    /// Every check, in [`DOCTOR_CHECKS`] order, so a clean report still says what it looked at.
    pub checks: Vec<CheckResult>,
    /// Every finding, grouped by check in that same order.
    pub findings: Vec<DoctorFinding>,
}

/// How a diagnosis was asked to behave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DoctorOptions {
    /// Resolve from the catalog cache alone, failing rather than fetching.
    pub offline: bool,
}

/// The findings that decide the exit code.
pub fn doctor_failures(report: &DoctorReport) -> Vec<DoctorFinding> {
    let _ = report;
    todo!("port project/doctor.ts:doctorFailures")
}

/// The findings that are reported and nothing more.
pub fn doctor_warnings(report: &DoctorReport) -> Vec<DoctorFinding> {
    let _ = report;
    todo!("port project/doctor.ts:doctorWarnings")
}

/// Whether every check passed: no failures. A warning leaves a project healthy.
pub fn is_healthy(report: &DoctorReport) -> bool {
    let _ = report;
    todo!("port project/doctor.ts:isHealthy")
}

/// Runs every check against a project.
///
/// `env` is the same environment `plan_install` resolves with, so the `expects` check cannot
/// contradict what install would write.
///
/// # Errors
///
/// Exit 2 for a malformed config or catalog, an unknown harness, an unreadable state file, or an
/// artifact that cannot be inspected; exit 3 for a resolution error; exit 4 if a fetch fails, or
/// under `--offline` when the cache cannot answer. A finding is never an error.
pub fn diagnose_project(
    project_dir: &Path,
    env: &Env,
    options: DoctorOptions,
) -> Result<DoctorReport> {
    let _ = (project_dir, env, options);
    todo!("port project/doctor.ts:diagnoseProject")
}
