use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::{IndexMap, IndexSet};
use serde_json::json;

use crate::errors::{Result, config_error};
use crate::harness::adapter::{
    AppliedArtifact, HarnessAdapter, InstallScope, PlannedArtifact, ProjectPaths, SkippedHook,
};
use crate::harness::definitions::PROFILES;
use crate::harness::profile::{ProfileAdapter, adapter_for};
use crate::model::catalog::{CatalogLoadOptions, load_catalogs, merge_catalogs};
use crate::model::config::{ProjectConfig, Trust, load_project_config};
use crate::model::documents::DocumentShape;
use crate::model::git::RefreshMode;
use crate::model::sources::SourceContext;
use crate::model::state::{
    ArtifactMode, OwnedArtifact, STATE_VERSION, State, read_state, write_state,
};
use crate::project::audit::{
    AuditFinding, AuditItems, audit_items, catalog_roots, refuse_failures,
};
use crate::project::exec::{ExecChange, refuse_unaccepted, review_exec};
use crate::project::gitignore::{GitignoreStatus, gitignore_status, write_gitignore_blocks};
use crate::project::lock::{
    Lock, assert_lock_current, build_lock, item_digests, read_catalog_pins, read_lock_text,
    read_locked_items, serialize_lock, verify_digests, write_lock_text,
};
use crate::project::ownership::{OwnershipOptions, authorize_plan};
use crate::project::prune::{PrunedArtifact, plan_prune, prune_artifacts};
use crate::resolution::resolve::{Bundle, resolve_bundle};
use crate::util::cmp::js_cmp;
use crate::util::env::{Env, home_dir};
use crate::util::json::{self, JsonValue};
use crate::util::path::{join, normalize};

pub static ADAPTERS: LazyLock<IndexMap<&'static str, ProfileAdapter>> = LazyLock::new(|| {
    PROFILES
        .iter()
        .map(|&profile| (profile.name, adapter_for(profile)))
        .collect()
});

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstallOptions {
    pub frozen: bool,
    pub offline: bool,
    pub adopt: bool,
    pub mode: Option<ArtifactMode>,
    pub no_audit: bool,
    pub accept_exec: bool,
}

#[derive(Clone)]
pub struct AdapterPlan {
    pub adapter: &'static dyn HarnessAdapter,
    pub plan: Vec<PlannedArtifact>,
}

#[derive(Clone)]
pub struct PlannedInstall {
    pub bundle: Bundle,
    pub harnesses: Vec<String>,
    pub plans: Vec<AdapterPlan>,
    pub artifacts: Vec<PlannedArtifact>,
    pub skipped: Vec<SkippedHook>,
    pub prior: State,
    pub lock: Lock,
    pub lock_text: String,
    pub roots: IndexMap<String, PathBuf>,
    pub trust: IndexMap<String, Trust>,
    pub project: ProjectPaths,
}

#[derive(Clone, Debug)]
pub struct InstallPreview {
    pub bundle: Bundle,
    pub harnesses: Vec<String>,
    pub artifacts: Vec<PlannedArtifact>,
    pub skipped: Vec<SkippedHook>,
    pub pruned: Vec<PrunedArtifact>,
    pub lock_changed: bool,
    pub gitignore: Vec<GitignoreStatus>,
    pub audit: Vec<AuditFinding>,
    pub endpoints: Vec<ExecChange>,
}

#[derive(Clone, Debug)]
pub struct InstallResult {
    pub bundle: Bundle,
    pub harnesses: Vec<String>,
    pub artifacts: Vec<AppliedArtifact>,
    pub skipped: Vec<SkippedHook>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub pruned: Vec<PrunedArtifact>,
    pub audit: Vec<AuditFinding>,
    pub endpoints: Vec<ExecChange>,
}

pub const USER_PROJECT_DIRNAME: &str = ".ambit";

pub fn user_project_dir(env: &Env) -> Option<PathBuf> {
    home_dir(env).map(|home| join(&home, USER_PROJECT_DIRNAME))
}

pub fn project_paths(project_dir: &Path, env: &Env, mode: Option<ArtifactMode>) -> ProjectPaths {
    if let Some(home) = home_dir(env)
        && normalize(project_dir) == normalize(&join(&home, USER_PROJECT_DIRNAME))
    {
        return ProjectPaths {
            root: home,
            scope: Some(InstallScope::User),
            mode,
        };
    }

    ProjectPaths {
        root: project_dir.to_path_buf(),
        scope: Some(InstallScope::Project),
        mode,
    }
}

pub fn ignored_artifacts<'a>(
    project: &ProjectPaths,
    artifacts: &'a [OwnedArtifact],
) -> &'a [OwnedArtifact] {
    if project.scope == Some(InstallScope::User) {
        &[]
    } else {
        artifacts
    }
}

pub fn adapters_for(harnesses: &[String]) -> Result<Vec<&'static dyn HarnessAdapter>> {
    let adapters: &'static IndexMap<&'static str, ProfileAdapter> = &ADAPTERS;

    harnesses
        .iter()
        .map(|name| {
            if let Some(adapter) = adapters.get(name.as_str()) {
                return Ok(adapter as &'static dyn HarnessAdapter);
            }

            let mut known: Vec<&str> = adapters.keys().copied().collect();

            known.sort_by(|a, b| js_cmp(a, b));

            Err(config_error(
                format!("unknown harness \"{name}\" (ambit.yml)"),
                [
                    format!("this build ships adapters for: {}", known.join(", ")),
                    "remove it from `harnesses`, or correct the spelling".to_owned(),
                ],
            ))
        })
        .collect()
}

fn identity_of(artifact: &PlannedArtifact) -> String {
    let PlannedArtifact::HarnessConfig(config) = artifact else {
        return artifact.path().to_owned();
    };

    let entries: Vec<JsonValue> = config
        .entries
        .iter()
        .map(|entry| json!({ "key": entry.key, "value": entry.value }))
        .collect();

    json::stringify(&json!([
        config.path,
        config.section,
        config.format.as_str(),
        config.shape.map(DocumentShape::as_str),
        config.root_defaults,
        entries,
    ]))
}

pub fn plan_for(
    adapters: &[&'static dyn HarnessAdapter],
    bundle: &Bundle,
    project: &ProjectPaths,
) -> Vec<AdapterPlan> {
    let mut claimed: IndexSet<String> = IndexSet::new();

    adapters
        .iter()
        .map(|&adapter| AdapterPlan {
            adapter,
            plan: adapter
                .plan(bundle, project)
                .into_iter()
                .filter(|artifact| claimed.insert(identity_of(artifact)))
                .collect(),
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanContext {
    pub refresh: Option<RefreshMode>,
    pub released: Vec<String>,
}

struct CatalogPlan {
    pins: IndexMap<String, String>,
    refresh: Option<IndexMap<String, RefreshMode>>,
}

fn catalog_plan(
    project_dir: &Path,
    config: &ProjectConfig,
    options: InstallOptions,
    plan: &PlanContext,
) -> Result<CatalogPlan> {
    let released: IndexSet<&str> = plan.released.iter().map(String::as_str).collect();
    let recorded = read_catalog_pins(project_dir, config)?;
    let pins: IndexMap<String, String> = recorded
        .into_iter()
        .filter(|(name, _)| !released.contains(name.as_str()))
        .collect();

    let Some(mode) = plan.refresh.filter(|_| !options.offline) else {
        return Ok(CatalogPlan {
            pins,
            refresh: None,
        });
    };

    let asking: IndexMap<String, RefreshMode> = config
        .catalogs
        .iter()
        .map(|entry| entry.name.as_str())
        .filter(|name| !pins.contains_key(*name) && !released.contains(name))
        .map(|name| (name.to_owned(), mode))
        .collect();

    Ok(CatalogPlan {
        pins,
        refresh: if asking.is_empty() {
            None
        } else {
            Some(asking)
        },
    })
}

pub fn plan_install(
    project_dir: &Path,
    env: &Env,
    options: InstallOptions,
    plan: &PlanContext,
) -> Result<PlannedInstall> {
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

    let CatalogPlan { pins, refresh } = catalog_plan(project_dir, &config, options, plan)?;
    let loaded = load_catalogs(
        &config,
        &context,
        &mut CatalogLoadOptions {
            collect: None,
            refresh,
            pins: Some(pins),
        },
    )?;
    let bundle = resolve_bundle(&config, &merge_catalogs(&loaded))?;

    let lock = build_lock(&loaded, &bundle, &item_digests(&bundle)?)?;
    let project = project_paths(project_dir, env, options.mode);

    let plans = plan_for(&adapters, &bundle, &project);
    let artifacts = plans
        .iter()
        .flat_map(|adapter_plan| adapter_plan.plan.iter().cloned())
        .collect();

    let skipped = adapters
        .iter()
        .flat_map(|adapter| adapter.skips(&bundle))
        .collect();
    let trust = config
        .catalogs
        .iter()
        .map(|entry| (entry.name.clone(), entry.trust))
        .collect();

    Ok(PlannedInstall {
        harnesses,
        plans,
        artifacts,
        skipped,
        prior: read_state(project_dir)?,
        lock_text: serialize_lock(&lock),
        lock,
        roots: catalog_roots(&loaded),
        trust,
        bundle,
        project,
    })
}

struct PlanWarnings {
    audit: Vec<AuditFinding>,
    endpoints: Vec<ExecChange>,
}

fn check_plan(
    project_dir: &Path,
    planned: &PlannedInstall,
    options: InstallOptions,
) -> Result<PlanWarnings> {
    let previous = read_locked_items(project_dir)?;

    if let Some(previous) = &previous {
        verify_digests(previous, &planned.lock)?;
    }

    if options.frozen {
        assert_lock_current(project_dir, &planned.lock_text)?;
    }

    let endpoints = if options.frozen {
        Vec::new()
    } else {
        let review = review_exec(
            previous.as_ref(),
            &planned.lock,
            &planned.bundle,
            &planned.trust,
        );

        if !options.accept_exec {
            refuse_unaccepted(&review.gated)?;
        }

        review.endpoints
    };

    if options.no_audit {
        return Ok(PlanWarnings {
            audit: Vec::new(),
            endpoints,
        });
    }

    let report = audit_items(AuditItems::of_bundle(&planned.bundle), &planned.roots)?;

    refuse_failures(&report)?;
    Ok(PlanWarnings {
        audit: report.warnings().cloned().collect(),
        endpoints,
    })
}

pub fn preview_install(
    project_dir: &Path,
    env: &Env,
    options: InstallOptions,
) -> Result<InstallPreview> {
    let planned = plan_install(
        project_dir,
        env,
        options,
        &PlanContext {
            refresh: Some(RefreshMode::Probe),
            released: Vec::new(),
        },
    )?;

    let warnings = check_plan(project_dir, &planned, options)?;

    authorize_plan(
        &planned.artifacts,
        &planned.prior,
        OwnershipOptions {
            adopt: options.adopt,
        },
    )?;

    let pruned = plan_prune(&planned.artifacts, &planned.prior)?;
    let owned: Vec<OwnedArtifact> = planned.artifacts.iter().map(OwnedArtifact::from).collect();
    let gitignore = gitignore_status(project_dir, ignored_artifacts(&planned.project, &owned))?;
    let lock_changed = read_lock_text(project_dir)?.as_deref() != Some(planned.lock_text.as_str());

    Ok(InstallPreview {
        bundle: planned.bundle,
        harnesses: planned.harnesses,
        artifacts: planned.artifacts,
        skipped: planned.skipped,
        pruned,
        lock_changed,
        gitignore,
        audit: warnings.audit,
        endpoints: warnings.endpoints,
    })
}

pub fn install_project(
    project_dir: &Path,
    env: &Env,
    options: InstallOptions,
    released: &[String],
) -> Result<InstallResult> {
    let planned = plan_install(
        project_dir,
        env,
        options,
        &PlanContext {
            refresh: Some(RefreshMode::Advance),
            released: released.to_vec(),
        },
    )?;

    let warnings = check_plan(project_dir, &planned, options)?;

    let owner = authorize_plan(
        &planned.artifacts,
        &planned.prior,
        OwnershipOptions {
            adopt: options.adopt,
        },
    )?;

    let mut artifacts: Vec<AppliedArtifact> = Vec::new();

    for adapter_plan in &planned.plans {
        artifacts.extend(adapter_plan.adapter.apply(&adapter_plan.plan, &owner)?);
    }

    let pruned = prune_artifacts(&planned.project.root, &planned.artifacts, &planned.prior)?;

    write_lock_text(project_dir, &planned.lock_text)?;
    write_state(
        project_dir,
        &State {
            version: STATE_VERSION,
            harnesses: planned.harnesses.clone(),
            artifacts: artifacts.clone(),
        },
    )?;

    // Must stay after `write_state`: failing before it would leave installed artifacts unowned.
    write_gitignore_blocks(project_dir, ignored_artifacts(&planned.project, &artifacts))?;

    Ok(InstallResult {
        bundle: planned.bundle,
        harnesses: planned.harnesses,
        artifacts,
        skipped: planned.skipped,
        pruned,
        audit: warnings.audit,
        endpoints: warnings.endpoints,
    })
}

#[cfg(test)]
pub(crate) mod fixture;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod harnesses_tests;
