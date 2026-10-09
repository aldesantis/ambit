use std::path::Path;

use indexmap::{IndexMap, IndexSet};

use crate::errors::{AmbitError, ExitCode, Result, at, config_error};
use crate::model::catalog::{Catalog, CatalogLoadOptions, load_catalogs, merge_catalogs};
use crate::model::config::{ProjectConfig, load_project_config};
use crate::model::git::RefreshMode;
use crate::model::lock_file::read_catalog_pins;
use crate::model::sources::SourceContext;
use crate::model::state::ArtifactMode;
use crate::project::bundle_diff::{BundleDiff, diff_bundles};
use crate::project::install::{InstallOptions, InstallResult, install_project};
use crate::resolution::resolve::{Bundle, resolve_bundle};
use crate::util::env::Env;
use crate::util::string_enum;

#[cfg(test)]
mod tests;

string_enum! {
    pub enum CatalogFreshness {
        Outdated => "outdated",
        Current => "current",
        Pinned => "pinned",
        Unversioned => "unversioned",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPin {
    pub name: String,
    pub source: String,
    pub r#ref: Option<String>,
    pub freshness: CatalogFreshness,
    pub commit: Option<String>,
    pub latest: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdatePlan {
    pub catalogs: Vec<CatalogPin>,
    pub diff: BundleDiff,
}

#[derive(Clone, Debug)]
pub struct UpdateResult {
    pub catalogs: Vec<CatalogPin>,
    pub diff: BundleDiff,
    pub install: InstallResult,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateOptions {
    pub catalogs: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateInstallOptions {
    pub adopt: bool,
    pub mode: Option<ArtifactMode>,
    pub no_audit: bool,
    pub accept_exec: bool,
}

pub fn catalogs_outdated(catalogs: &[CatalogPin]) -> bool {
    catalogs
        .iter()
        .any(|catalog| catalog.freshness == CatalogFreshness::Outdated)
}

fn unknown_catalog(name: &str, config: &ProjectConfig) -> AmbitError {
    let configured: Vec<&str> = config
        .catalogs
        .iter()
        .map(|catalog| catalog.name.as_str())
        .collect();
    let message = format!(
        "unknown catalog \"{name}\" {}",
        at(&config.origin.file, None)
    );

    if configured.is_empty() {
        return config_error(
            message,
            [
                "this project configures no catalogs at all",
                "add one under `catalogs`, then run the command again",
            ],
        );
    }

    config_error(
        message,
        [
            format!("this project configures: {}", configured.join(", ")),
            "correct the name, or omit it to take every catalog".to_owned(),
        ],
    )
}

fn refresh_plan(
    config: &ProjectConfig,
    named: &[String],
    mode: RefreshMode,
) -> Result<IndexMap<String, RefreshMode>> {
    let configured: IndexSet<&str> = config
        .catalogs
        .iter()
        .map(|catalog| catalog.name.as_str())
        .collect();

    if named.is_empty() {
        return Ok(configured
            .into_iter()
            .map(|name| (name.to_owned(), mode))
            .collect());
    }

    let mut plan = IndexMap::new();

    for name in named {
        if !configured.contains(name.as_str()) {
            return Err(unknown_catalog(name, config));
        }

        plan.insert(name.clone(), mode);
    }

    Ok(plan)
}

struct Resolution {
    catalogs: Vec<Catalog>,
    bundle: Bundle,
}

struct PlannedUpdate {
    plan: UpdatePlan,
    released: Vec<String>,
}

fn resolve_with(
    config: &ProjectConfig,
    context: &SourceContext,
    pins: &IndexMap<String, String>,
    refresh: Option<&IndexMap<String, RefreshMode>>,
) -> Result<Resolution> {
    let mut options = CatalogLoadOptions {
        pins: Some(pins.clone()),
        refresh: refresh.cloned(),
        ..CatalogLoadOptions::default()
    };
    let catalogs = load_catalogs(config, context, &mut options)?;
    let bundle = resolve_bundle(config, &merge_catalogs(&catalogs))?;

    Ok(Resolution { catalogs, bundle })
}

fn resolve_before(
    config: &ProjectConfig,
    context: &SourceContext,
    pins: &IndexMap<String, String>,
) -> Result<Option<Resolution>> {
    match resolve_with(config, context, pins, None) {
        Ok(resolution) => Ok(Some(resolution)),
        Err(error) if error.code != ExitCode::Network => Ok(None),
        Err(error) => Err(error),
    }
}

fn pin_of(before: &Catalog, after: Option<&Catalog>) -> CatalogPin {
    let base = CatalogPin {
        name: before.name.clone(),
        source: before.source.clone(),
        r#ref: before.r#ref.clone(),
        freshness: CatalogFreshness::Unversioned,
        commit: None,
        latest: None,
    };

    let (Some(commit), Some(after), Some(latest)) = (
        before.commit.as_ref(),
        after,
        after.and_then(|after| after.commit.as_ref()),
    ) else {
        return base;
    };

    let freshness = if after.moving == Some(false) {
        CatalogFreshness::Pinned
    } else if commit == latest {
        CatalogFreshness::Current
    } else {
        CatalogFreshness::Outdated
    };

    CatalogPin {
        freshness,
        commit: Some(commit.clone()),
        latest: Some(latest.clone()),
        ..base
    }
}

fn unresolved_pin_of(after: &Catalog) -> CatalogPin {
    let base = CatalogPin {
        name: after.name.clone(),
        source: after.source.clone(),
        r#ref: after.r#ref.clone(),
        freshness: CatalogFreshness::Unversioned,
        commit: None,
        latest: None,
    };

    let Some(latest) = after.commit.clone() else {
        return base;
    };

    let freshness = if after.moving == Some(false) {
        CatalogFreshness::Pinned
    } else {
        CatalogFreshness::Outdated
    };

    CatalogPin {
        freshness,
        latest: Some(latest),
        ..base
    }
}

fn plan_update(
    project_dir: &Path,
    env: &Env,
    mode: RefreshMode,
    options: &UpdateOptions,
) -> Result<PlannedUpdate> {
    let config = load_project_config(project_dir)?;
    let refresh = refresh_plan(&config, &options.catalogs, mode)?;
    let context = SourceContext {
        project_dir: project_dir.to_path_buf(),
        env: env.clone(),
        offline: false,
    };
    let pins = read_catalog_pins(project_dir, &config)?;

    // Must run before the refreshing pass: under `Advance` it moves the cache's refs.
    let before = resolve_before(&config, &context, &pins)?;
    let after = resolve_with(&config, &context, &pins, Some(&refresh))?;

    let catalogs = match &before {
        None => after.catalogs.iter().map(unresolved_pin_of).collect(),
        Some(before) => before
            .catalogs
            .iter()
            .map(|catalog| {
                let latest = after
                    .catalogs
                    .iter()
                    .find(|candidate| candidate.name == catalog.name);

                pin_of(catalog, latest)
            })
            .collect(),
    };
    let nothing = Bundle::default();
    let diff = diff_bundles(
        before
            .as_ref()
            .map_or(&nothing, |resolution| &resolution.bundle),
        &after.bundle,
    )?;

    Ok(PlannedUpdate {
        plan: UpdatePlan { catalogs, diff },
        released: refresh.into_keys().collect(),
    })
}

pub fn check_outdated(
    project_dir: &Path,
    env: &Env,
    options: &UpdateOptions,
) -> Result<UpdatePlan> {
    Ok(plan_update(project_dir, env, RefreshMode::Probe, options)?.plan)
}

pub fn preview_update(
    project_dir: &Path,
    env: &Env,
    options: &UpdateOptions,
) -> Result<UpdatePlan> {
    check_outdated(project_dir, env, options)
}

pub fn update_project(
    project_dir: &Path,
    env: &Env,
    options: &UpdateOptions,
    install: UpdateInstallOptions,
) -> Result<UpdateResult> {
    let PlannedUpdate { plan, released } =
        plan_update(project_dir, env, RefreshMode::Advance, options)?;
    let installed = install_project(
        project_dir,
        env,
        InstallOptions {
            adopt: install.adopt,
            mode: install.mode,
            no_audit: install.no_audit,
            accept_exec: install.accept_exec,
            ..InstallOptions::default()
        },
        &released,
    )?;

    Ok(UpdateResult {
        catalogs: plan.catalogs,
        diff: plan.diff,
        install: installed,
    })
}
