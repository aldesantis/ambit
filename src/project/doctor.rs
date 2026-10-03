//! `ambit doctor`: preconditions, drift, and ownership.
//!
//! `validate` checks whether the catalog is coherent; `status` checks whether install would change
//! an artifact. This command covers what neither does: an unset environment variable a skill
//! needs, a committed lock that no longer matches what resolution produces, and artifacts a crashed
//! install left present but unowned.
//!
//! Reports findings instead of returning errors, like `validate`. Every check runs, the whole list
//! is printed, and the exit code carries the verdict (exit 6).
//!
//! Two severities; only failures reach exit 6. Per spec §5, an uninterpolated `${VAR}` must not
//! fail an install, so a missing environment variable is a failure here: install can't refuse it,
//! so this command must catch it. A materialization mode mismatch (`--copy` vs `--link`) is a
//! warning: both put identical bytes in front of the harness, and it's a per-run choice with
//! nothing persisting it, so `status` ignores mode entirely too. A harness limitation ambit can't
//! write around is a warning for the same reason.
//!
//! Runs `plan_install` once and derives everything else from it, so `doctor` can't disagree with
//! `status` about an artifact or with `install --frozen` about the lock. Nothing here writes.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use indexmap::IndexMap;

use crate::errors::Result;
use crate::harness::adapter::{PlannedArtifact, PlannedHarnessConfig};
use crate::harness::definitions::CODEX;
use crate::harness::env::referenced_names;
use crate::model::catalog::MergedMcp;
use crate::model::documents::managed_key;
use crate::model::expectation::expected_env;
use crate::model::lock_file::{LOCK_FILENAME, read_lock_text};
use crate::model::mcp_entity::McpTransport;
use crate::model::state::{ArtifactMode, OwnedArtifact};
use crate::project::gitignore::gitignore_status;
use crate::project::install::{InstallOptions, PlanContext, plan_install};
use crate::project::status::{ArtifactState, StatusArtifact, status_of_plan};
use crate::resolution::resolve::Bundle;
use crate::util::cmp::js_cmp;
use crate::util::env::Env;
use crate::util::fs::{EntryKind, lstat_kind};
use crate::util::string_enum;

string_enum! {
    /// The checks, in the order they run and are reported: the world around the project, then the
    /// record of the last install, then what that record and the project disagree about, then the
    /// two that are merely worth knowing (materialization mode, harness limitations).
    ///
    /// `Expects` covers every kind of precondition an entity can declare, not one check per kind,
    /// since they share a verdict and a fix: something about the machine isn't as the catalog
    /// needs, and the reader sets it. Today that's `env:` alone; `bin:` would be a case inside this
    /// check, not a separate entry.
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
    report
        .findings
        .iter()
        .filter(|finding| finding.severity == DoctorSeverity::Fail)
        .cloned()
        .collect()
}

/// The findings that are reported and nothing more.
pub fn doctor_warnings(report: &DoctorReport) -> Vec<DoctorFinding> {
    report
        .findings
        .iter()
        .filter(|finding| finding.severity == DoctorSeverity::Warn)
        .cloned()
        .collect()
}

/// Whether every check passed: no failures. A warning leaves a project healthy.
pub fn is_healthy(report: &DoctorReport) -> bool {
    !report
        .findings
        .iter()
        .any(|finding| finding.severity == DoctorSeverity::Fail)
}

/// The line a Codex user has to have in their own config for any hook ambit writes to run.
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

/// What wants each environment variable: one line per declarer, in a fixed order (skills, servers,
/// hooks, then config references), keyed by variable.
type EnvDemands = IndexMap<String, Vec<String>>;

/// Every string inside a map of strings, in key order.
///
/// Keys are sorted so the discovery order depends only on the value, not on how it was built.
fn strings_in(map: &IndexMap<String, String>) -> Vec<String> {
    let mut entries: Vec<(&String, &String)> = map.iter().collect();

    entries.sort_by(|(a, _), (b, _)| js_cmp(a, b));
    entries
        .into_iter()
        .map(|(_, value)| value.clone())
        .collect()
}

/// Every variable one MCP entity's own strings reference: its url and headers, or its arguments
/// and the values its env map supplies.
///
/// Read off the entity rather than the installed server, because each harness spells a reference
/// in its own syntax, and the entity is where the answer is the same for all of them.
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

/// Who wants each environment variable the bundle needs.
///
/// Four routes: a skill's `env:` expectation (read by the agent at runtime), a server's `env:`
/// expectation (read by the server), a hook's `env:` expectation (read by the command the harness
/// spawns), and a `${VAR}` reference in a config file (expanded by the harness when it spawns the
/// server). The fix is the same for all four: set the variable. Ambit writes references into these
/// files, not values, so nothing needs reinstalling once it is set.
///
/// The fourth route is why this check reads more than `expects`: an author can reference `${VAR}`
/// in a transport's headers or in the value an entry of its env map supplies without declaring it,
/// and this is the only check that sees it.
///
/// A hook contributes through its `expects` alone, never a config-reference line: a `${VAR}` inside
/// a hook's `command` is left intact for the shell the harness spawns, not rewritten into a
/// harness's own reference syntax, so there is no reference in the file to find.
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

    // Read off the entity rather than the config file's bytes. Each harness spells a reference in
    // its own syntax (`${VAR}`, `${env:VAR}`, `{env:VAR}`, Codex's bare variable name under
    // `env_http_headers`), so scanning the written file would miss it for four of the five
    // harnesses.
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

/// Every variable something in the bundle expects that the environment does not have.
///
/// "Does not have" means strictly absent, not empty. An empty value is a decision someone made,
/// and it's what install interpolates; treating it as missing would flag a project configured
/// exactly as its author intended.
///
/// Reported in variable order, so the list depends on the bundle, not on check order.
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

/// Whether the committed lock is what resolution now produces: `--frozen`'s question, asked
/// without failing an install over it.
///
/// Compared as bytes, exactly as `--frozen` does; the lock is a record nothing parses. Deliberately
/// not `assert_lock_current`, whose message names `--frozen` and the project's absolute path,
/// neither of which belongs in a report.
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

/// Artifacts that are present but that state does not claim: what install refuses.
///
/// State is written after the filesystem changes it describes, so an install that crashed leaves
/// its own artifacts present-but-unowned, and the next plain install stops on them. The fix is
/// `--adopt`.
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

/// The one concrete next step for an artifact install would change.
fn drift_step(state: ArtifactState) -> &'static str {
    match state {
        ArtifactState::Stale => "run `ambit install`, or `ambit prune`, to remove it",
        ArtifactState::Missing => "run `ambit install` to write it",
        _ => "run `ambit install` to restore it",
    }
}

/// A planned artifact in the shape state records it, which is what the `.gitignore` renderer
/// reads.
fn owned_of(artifact: &PlannedArtifact) -> OwnedArtifact {
    let config = match artifact {
        PlannedArtifact::HarnessConfig(config) => Some(config),
        _ => None,
    };

    OwnedArtifact {
        path: artifact.path().to_owned(),
        kind: artifact.kind(),
        mode: artifact.mode(),
        managed_keys: config.map(|config| config.managed_keys.clone()),
        format: config.map(|config| config.format),
        shape: config.and_then(|config| config.shape),
    }
}

/// Everything install would change about the project: `status`'s findings, plus the managed
/// `.gitignore` blocks, which `status` has no row for.
///
/// The `.gitignore` blocks carry no state entry (the markers are the record, in `gitignore.rs`),
/// so they can't be a `status` row, and nothing else checks them.
///
/// `Unowned` is excluded here: it belongs to the ownership check, and reporting it twice would
/// double every finding about a crashed install.
fn drift_findings(
    project_dir: &Path,
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

    // Same question install's `--dry-run` asks, of the same renderer that writes. Checked per file
    // because the two blocks go stale for different reasons: the nested one whenever the bundle
    // changes, the root one almost never.
    let owned: Vec<OwnedArtifact> = artifacts.iter().map(owned_of).collect();
    let gitignore = gitignore_status(project_dir, &owned)?;

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

/// Which mode a directory is installed in, read off the target rather than off state.
///
/// Uses `lstat` so a symlink is seen as itself. Errors are treated as "no answer" rather than
/// returned: this only runs on artifacts already read successfully, and a mode mismatch isn't
/// worth aborting the run over.
fn installed_mode(target: &Path) -> Option<ArtifactMode> {
    match lstat_kind(target) {
        Ok(EntryKind::Symlink) => Some(ArtifactMode::Link),
        Ok(EntryKind::Dir) => Some(ArtifactMode::Copy),
        _ => None,
    }
}

/// Directories installed in a mode a plain `ambit install` would not choose.
///
/// A warning, only on artifacts that are otherwise `ok`. `--copy` and `--link` are per-run flags
/// with nothing persisting them, so the divergence is permanent by design and both modes put the
/// same bytes in front of the harness; that's why `status` ignores mode entirely. Still worth
/// reporting, since the next plain install will silently swap these over.
///
/// Covers both directory kinds (skill and hook), since `status` is deliberately silent about mode
/// and this is the only check that reports it.
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

/// What a configured harness needs that ambit is not allowed to write.
///
/// One finding today, Codex's: hooks there are experimental and only read when a user's own
/// `config.toml` carries `[features] codex_hooks = true`. That file is user-level, not the
/// project's, so ambit writes `.codex/hooks.json` correctly and the hooks still never fire.
///
/// A warning, not a failure: ambit cannot tell whether the flag is set, and failing would leave
/// anyone on Codex with a `doctor` that can never pass. Only raised when the project selects a
/// hook.
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

/// Each check's verdict, derived from its findings so the two halves cannot disagree.
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

/// Runs every check against a project.
///
/// `env` is the same environment `plan_install` resolves with, so the `expects` check cannot
/// contradict what install would write. `project_dir` is the project root, absolute.
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
