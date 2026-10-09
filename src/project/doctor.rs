use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use indexmap::IndexMap;

use crate::errors::Result;
use crate::harness::adapter::{PlannedArtifact, PlannedHarnessConfig, ProjectPaths};
use crate::harness::definitions::CODEX;
use crate::harness::env::referenced_names;
use crate::model::catalog::MergedMcp;
use crate::model::documents::managed_key;
use crate::model::expectation::expected_env;
use crate::model::lock_file::{LOCK_FILENAME, read_lock_text};
use crate::model::mcp_entity::McpTransport;
use crate::model::state::{ArtifactMode, OwnedArtifact};
use crate::project::gitignore::gitignore_status;
use crate::project::install::{InstallOptions, PlanContext, ignored_artifacts, plan_install};
use crate::project::status::{ArtifactState, StatusArtifact, status_of_plan};
use crate::resolution::resolve::Bundle;
use crate::util::cmp::js_cmp;
use crate::util::env::Env;
use crate::util::fs::{EntryKind, lstat_kind};
use crate::util::string_enum;

string_enum! {
    pub enum DoctorCheck {
        Expects => "expects",
        Lock => "lock",
        Ownership => "ownership",
        Drift => "drift",
        Mode => "mode",
        Harness => "harness",
    }
}

pub const DOCTOR_CHECKS: &[DoctorCheck] = DoctorCheck::ALL;

string_enum! {
    pub enum DoctorSeverity {
        Fail => "fail",
        Warn => "warn",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoctorFinding {
    pub check: DoctorCheck,
    pub severity: DoctorSeverity,
    pub message: String,
    pub detail: Vec<String>,
}

string_enum! {
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
    pub checks: Vec<CheckResult>,
    pub findings: Vec<DoctorFinding>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DoctorOptions {
    pub offline: bool,
}

pub fn doctor_failures(report: &DoctorReport) -> Vec<DoctorFinding> {
    report
        .findings
        .iter()
        .filter(|finding| finding.severity == DoctorSeverity::Fail)
        .cloned()
        .collect()
}

pub fn doctor_warnings(report: &DoctorReport) -> Vec<DoctorFinding> {
    report
        .findings
        .iter()
        .filter(|finding| finding.severity == DoctorSeverity::Warn)
        .cloned()
        .collect()
}

pub fn is_healthy(report: &DoctorReport) -> bool {
    !report
        .findings
        .iter()
        .any(|finding| finding.severity == DoctorSeverity::Fail)
}

const CODEX_HOOKS_FEATURE: &str = "[features] codex_hooks = true";

fn fail(check: DoctorCheck, message: String, detail: Vec<String>) -> DoctorFinding {
    DoctorFinding {
        check,
        severity: DoctorSeverity::Fail,
        message,
        detail,
    }
}

fn warn(check: DoctorCheck, message: String, detail: Vec<String>) -> DoctorFinding {
    DoctorFinding {
        check,
        severity: DoctorSeverity::Warn,
        message,
        detail,
    }
}

type EnvDemands = IndexMap<String, Vec<String>>;

fn strings_in(map: &IndexMap<String, String>) -> Vec<String> {
    let mut entries: Vec<(&String, &String)> = map.iter().collect();

    entries.sort_by(|(a, _), (b, _)| js_cmp(a, b));
    entries
        .into_iter()
        .map(|(_, value)| value.clone())
        .collect()
}

fn entity_references(mcp: &MergedMcp) -> Vec<String> {
    let (strings, bearer) = match &mcp.transport {
        McpTransport::Http(http) => {
            let mut strings = vec![http.url.clone()];

            strings.extend(strings_in(&http.headers));
            (strings, http.bearer_token_env_var.clone())
        }
        McpTransport::Stdio(stdio) => {
            let mut strings = stdio.args.clone();

            strings.extend(strings_in(&stdio.env));
            (strings, None)
        }
    };

    strings
        .iter()
        .flat_map(|value| referenced_names(value))
        .chain(bearer)
        .collect()
}

fn sorted_unique(values: Vec<String>) -> Vec<String> {
    let mut unique: Vec<String> = values
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();

    unique.sort_by(|a, b| js_cmp(a, b));
    unique
}

fn env_demands(bundle: &Bundle, artifacts: &[PlannedArtifact]) -> EnvDemands {
    let mut demands = EnvDemands::new();
    let mut want = |variable: String, line: String| demands.entry(variable).or_default().push(line);

    for skill in &bundle.skills {
        for variable in expected_env(&skill.expects) {
            want(variable, format!("skill \"{}\" expects it", skill.name));
        }
    }

    for mcp in &bundle.mcps {
        for variable in expected_env(&mcp.expects) {
            want(variable, format!("MCP server \"{}\" expects it", mcp.name));
        }
    }

    for hook in &bundle.hooks {
        for variable in expected_env(&hook.expects) {
            want(variable, format!("hook \"{}\" expects it", hook.name));
        }
    }

    let referenced: IndexMap<&str, Vec<String>> = bundle
        .mcps
        .iter()
        .map(|mcp| (mcp.name.as_str(), sorted_unique(entity_references(mcp))))
        .collect();
    let configs = artifacts.iter().filter_map(|artifact| match artifact {
        PlannedArtifact::HarnessConfig(config) => Some(config),
        _ => None,
    });

    for artifact in configs {
        let PlannedHarnessConfig {
            section,
            entries,
            path,
            ..
        } = artifact;

        for entry in entries {
            let key = managed_key(section, &entry.key);

            for variable in referenced.get(entry.key.as_str()).into_iter().flatten() {
                want(
                    variable.clone(),
                    format!(
                        "\"{key}\" in {path} references it, for the harness to expand at spawn"
                    ),
                );
            }
        }
    }

    demands
}

fn expect_findings(
    bundle: &Bundle,
    artifacts: &[PlannedArtifact],
    env: &Env,
) -> Vec<DoctorFinding> {
    let demands = env_demands(bundle, artifacts);
    let mut variables: Vec<&String> = demands.keys().collect();

    variables.sort_by(|a, b| js_cmp(a, b));
    variables
        .into_iter()
        .filter(|variable| !env.contains_key(variable.as_str()))
        .map(|variable| {
            let mut detail = demands[variable].clone();

            detail.push(format!(
                "set {variable} in the environment the agent runs in"
            ));
            fail(
                DoctorCheck::Expects,
                format!("unset environment variable \"{variable}\""),
                detail,
            )
        })
        .collect()
}

fn lock_findings(project_dir: &Path, expected: &str) -> Result<Vec<DoctorFinding>> {
    let actual = read_lock_text(project_dir)?;

    match actual.as_deref() {
        Some(text) if text == expected => Ok(Vec::new()),
        None => Ok(vec![fail(
            DoctorCheck::Lock,
            format!("{LOCK_FILENAME} is missing"),
            vec![
                "resolution produces a lock, and this project has none recorded".to_owned(),
                "run `ambit install` to write it".to_owned(),
            ],
        )]),
        Some(_) => Ok(vec![fail(
            DoctorCheck::Lock,
            format!("{LOCK_FILENAME} is out of date"),
            vec![
                format!(
                    "resolving this project produces a different {LOCK_FILENAME} than the one on disk"
                ),
                format!(
                    "{LOCK_FILENAME} is written by `ambit install` and `ambit prune`, so config or a catalog commit has moved since the last one"
                ),
                "run `ambit install` to rewrite it".to_owned(),
            ],
        )]),
    }
}

fn ownership_findings(artifacts: &[StatusArtifact]) -> Vec<DoctorFinding> {
    artifacts
        .iter()
        .filter(|artifact| artifact.state == ArtifactState::Unowned)
        .map(|artifact| {
            fail(
                DoctorCheck::Ownership,
                format!("ambit does not own {}", artifact.path),
                vec![
                    artifact.detail.clone(),
                    "an `ambit install` that crashed leaves this: state is written after the files it describes".to_owned(),
                    "move it aside, or run `ambit install --adopt` to take ownership".to_owned(),
                ],
            )
        })
        .collect()
}

fn drift_step(state: ArtifactState) -> &'static str {
    match state {
        ArtifactState::Stale => "run `ambit install`, or `ambit prune`, to remove it",
        ArtifactState::Missing => "run `ambit install` to write it",
        _ => "run `ambit install` to restore it",
    }
}

fn drift_findings(
    project_dir: &Path,
    project: &ProjectPaths,
    artifacts: &[PlannedArtifact],
    status: &[StatusArtifact],
) -> Result<Vec<DoctorFinding>> {
    let mut findings: Vec<DoctorFinding> = status
        .iter()
        .filter(|artifact| {
            artifact.state != ArtifactState::Ok && artifact.state != ArtifactState::Unowned
        })
        .map(|artifact| {
            fail(
                DoctorCheck::Drift,
                format!("{} is {}", artifact.path, artifact.state),
                vec![
                    artifact.detail.clone(),
                    drift_step(artifact.state).to_owned(),
                ],
            )
        })
        .collect();

    let owned: Vec<OwnedArtifact> = artifacts.iter().map(OwnedArtifact::from).collect();
    let gitignore = gitignore_status(project_dir, ignored_artifacts(project, &owned))?;

    findings.extend(gitignore.into_iter().filter(|block| block.changed).map(|block| {
        fail(
            DoctorCheck::Drift,
            format!("{} does not hold the block install would write", block.file),
            vec![
                "ambit owns the lines between `# BEGIN ambit` and `# END ambit`, and rewrites them each install".to_owned(),
                "run `ambit install` to rewrite the block".to_owned(),
            ],
        )
    }));

    Ok(findings)
}

fn installed_mode(target: &Path) -> Option<ArtifactMode> {
    match lstat_kind(target) {
        Ok(EntryKind::Symlink) => Some(ArtifactMode::Link),
        Ok(EntryKind::Dir) => Some(ArtifactMode::Copy),
        _ => None,
    }
}

fn mode_findings(artifacts: &[PlannedArtifact], status: &[StatusArtifact]) -> Vec<DoctorFinding> {
    let matching: BTreeSet<&str> = status
        .iter()
        .filter(|artifact| artifact.state == ArtifactState::Ok)
        .map(|artifact| artifact.path.as_str())
        .collect();
    let directories = artifacts.iter().filter_map(|artifact| match artifact {
        PlannedArtifact::SkillDir(dir) | PlannedArtifact::HookDir(dir)
            if matching.contains(dir.path.as_str()) =>
        {
            Some(dir)
        }
        _ => None,
    });

    let mut findings = Vec::new();

    for artifact in directories {
        let Some(found) = installed_mode(&artifact.target) else {
            continue;
        };

        if found == artifact.mode {
            continue;
        }

        findings.push(warn(
            DoctorCheck::Mode,
            format!("{} is installed as a {found}", artifact.path),
            vec![
                if artifact.mode == ArtifactMode::Link {
                    "its source is a local directory someone edits, so a plain `ambit install` would symlink it".to_owned()
                } else {
                    "its source is pinned to a commit, so a plain `ambit install` would copy it"
                        .to_owned()
                },
                format!("keep passing `--{found}` to `ambit install` to leave it as it is"),
            ],
        ));
    }

    findings
}

fn harness_findings(bundle: &Bundle, harnesses: &[String]) -> Vec<DoctorFinding> {
    let codex = &*CODEX;
    let Some(layout) = codex.hooks.as_ref() else {
        return Vec::new();
    };

    if bundle.hooks.is_empty() || !harnesses.iter().any(|harness| harness == codex.name) {
        return Vec::new();
    }

    let name = codex.name;

    vec![warn(
        DoctorCheck::Harness,
        format!("{name} runs hooks only with `{CODEX_HOOKS_FEATURE}` set"),
        vec![
            format!(
                "this project selects {}, and ambit writes them to {}",
                if bundle.hooks.len() == 1 {
                    "a hook"
                } else {
                    "hooks"
                },
                layout.file
            ),
            format!(
                "{name}'s hooks are experimental, and the flag enabling them is user-level config ambit must not write"
            ),
            format!("set `{CODEX_HOOKS_FEATURE}` in your own {name} config to have them run"),
        ],
    )]
}

fn check_results(findings: &[DoctorFinding]) -> Vec<CheckResult> {
    DOCTOR_CHECKS
        .iter()
        .map(|&check| {
            let mut own = findings.iter().filter(|finding| finding.check == check);
            let status = if own
                .clone()
                .any(|finding| finding.severity == DoctorSeverity::Fail)
            {
                CheckStatus::Fail
            } else if own.next().is_none() {
                CheckStatus::Ok
            } else {
                CheckStatus::Warn
            };

            CheckResult { check, status }
        })
        .collect()
}

pub fn diagnose_project(
    project_dir: &Path,
    env: &Env,
    options: DoctorOptions,
) -> Result<DoctorReport> {
    let planned = plan_install(
        project_dir,
        env,
        InstallOptions {
            offline: options.offline,
            ..InstallOptions::default()
        },
        &PlanContext::default(),
    )?;
    let status = status_of_plan(&planned.artifacts, &planned.prior)?;

    let mut findings = expect_findings(&planned.bundle, &planned.artifacts, env);

    findings.extend(lock_findings(project_dir, &planned.lock_text)?);
    findings.extend(ownership_findings(&status.artifacts));
    findings.extend(drift_findings(
        project_dir,
        &planned.project,
        &planned.artifacts,
        &status.artifacts,
    )?);
    findings.extend(mode_findings(&planned.artifacts, &status.artifacts));
    findings.extend(harness_findings(&planned.bundle, &planned.harnesses));

    Ok(DoctorReport {
        checks: check_results(&findings),
        findings,
    })
}

#[cfg(test)]
mod tests;
