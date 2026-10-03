//! `ambit outdated` and `ambit update`: moving a pin, and finding out what moving it would cost.
//!
//! The lock records the commit each catalog resolved to, and every other command resolves to that
//! commit (`read_catalog_pins`), so these two are the only way a pin moves. Editing `ref:` by hand
//! and reinstalling also works, but it tells you nothing about what changed until you read the lock
//! diff afterwards.
//!
//! Both commands share one shape with one switch: resolve the project twice (once from the cache as
//! it stands, once with the named catalogs' refs resolved against the remote) and hand the two
//! bundles to [`diff_bundles`]. The *refresh mode* is what differs: `outdated` probes, leaving the
//! cache's refs untouched; `update` advances them, so the install that follows writes the new
//! commits into the lock.
//!
//! `update` installs by calling `install_project`, which resolves a third time. This is deliberate:
//! install is the only thing that knows how to install, and by the time it runs the cache already
//! holds the advanced refs, so it resolves to exactly what this module just reported without
//! touching the network again. A seam for handing it a pre-resolved bundle would save one catalog
//! parse at the cost of the guarantee that `ambit update` and `ambit install` install the same way.
//!
//! Every source a project has is a catalog, so naming catalogs to `update` covers everything there
//! is to move; no `ref:` is left outside its reach.

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
    /// Where one catalog stands against its own `ref`.
    ///
    /// - `Outdated`: the ref names a different commit than the project resolves to now.
    /// - `Current`: it names the same one.
    /// - `Pinned`: the `ref` is a commit, so there is nothing for it to name differently.
    /// - `Unversioned`: a `path:` source, which has no revision at all.
    ///
    /// `Unversioned` is a separate word from `Current` because the two are not the same claim: a
    /// directory's contents can change between runs exactly as a branch's can, but it has no way to
    /// be *behind*, since there's no other revision to be behind of. `ambit status` is the command
    /// that answers whether a `path:` catalog's bundle still matches what is installed.
    pub enum CatalogFreshness {
        Outdated => "outdated",
        Current => "current",
        Pinned => "pinned",
        Unversioned => "unversioned",
    }
}

/// One catalog, and where its pin stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPin {
    pub name: String,
    /// The `source` as config wrote it.
    pub source: String,
    /// The `ref` as config wrote it, absent when the entry named none: the source's default branch.
    pub r#ref: Option<String>,
    pub freshness: CatalogFreshness,
    /// The commit the project resolves to now. Absent for a `path:` source.
    pub commit: Option<String>,
    /// The commit the ref names on the remote. Absent for a `path:` source; equal to `commit`
    /// unless outdated.
    pub latest: Option<String>,
}

/// What both commands compute: where every pin stands, and what moving them would change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdatePlan {
    /// Every configured catalog, in config order, whether or not this run refreshed it.
    pub catalogs: Vec<CatalogPin>,
    pub diff: BundleDiff,
}

/// What `ambit update` did: the plan it acted on, and the install that carried it out.
#[derive(Clone, Debug)]
pub struct UpdateResult {
    pub catalogs: Vec<CatalogPin>,
    pub diff: BundleDiff,
    pub install: InstallResult,
}

/// How a check or an update was asked to behave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UpdateOptions {
    /// The catalogs to refresh, by name. Empty means every one of them.
    ///
    /// A name no catalog carries is an error rather than a no-op: `ambit update compnay` reporting
    /// "nothing to do" is a typo that looks like an answer.
    pub catalogs: Vec<String>,
}

/// Everything `update` passes through to the install it ends with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UpdateInstallOptions {
    pub adopt: bool,
    pub mode: Option<ArtifactMode>,
}

/// Whether any pin has somewhere to move: over a bare list of pins, since [`UpdatePlan`] and
/// [`UpdateResult`] carry the same list.
pub fn catalogs_outdated(catalogs: &[CatalogPin]) -> bool {
    catalogs
        .iter()
        .any(|catalog| catalog.freshness == CatalogFreshness::Outdated)
}

/// The error for a named catalog the project does not configure: exit 2 naming the configured
/// catalogs, since the fix is always one of them.
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

/// How each catalog may consult its remote: `mode` for the named ones, nothing for the rest.
///
/// One case this can't narrow: two catalogs can be two refs of one repository, sharing a clone.
/// Advancing either advances the clone, so `ambit update company` moves a sibling pointed at the
/// same repository too. The report still tells the truth: the sibling's row reads `outdated` and
/// its change appears in the diff.
///
/// # Errors
///
/// Exit 2 for a name no catalog carries.
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

/// One pass of the two below: what the catalogs were, and what they resolved to.
struct Resolution {
    catalogs: Vec<Catalog>,
    bundle: Bundle,
}

/// The report, plus which catalogs this run refreshed.
///
/// The second field isn't part of the report (nobody reading `ambit outdated` needs it), but it's
/// what the install at the end of `ambit update` must be told: those catalogs' lock pins are the
/// commits the update is replacing, and an install that honoured them would undo it. Kept internal
/// for that reason.
struct PlannedUpdate {
    plan: UpdatePlan,
    /// The catalogs whose pin this run moved past, by name.
    released: Vec<String>,
}

/// Resolves the project through one load, so the two passes below cannot differ in anything else.
///
/// Both passes are given the lock's pins; the refresh is what overrides them.
///
/// The `before` pass must be the commit the project resolves to today, which for a pinned catalog
/// is the locked commit, not whatever the shared clone's `refs/heads/main` happens to hold.
/// Otherwise the report's `commit` column would name a commit the project would not install. The
/// `after` pass gets the same pins plus the refresh plan on top; a refresh wins per catalog
/// (`fetch_git_source` ignores a pin unless it is resolving from the cache), so a named catalog is
/// answered by the remote and an unnamed one stays exactly where it was pinned. That's what makes
/// `ambit update company` a claim about `company` alone.
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

/// The `before` pass, with a cache it cannot resolve reported as nothing rather than returned as an
/// error.
///
/// This pass must not be fatal. A catalog can move on and leave the commit sitting in the cache
/// unresolvable (a skill now requiring a hook the older commit didn't ship, a name since
/// corrected). The project and the remote are both fine; only the stale copy is broken, and
/// `ambit update` is the command that replaces it. Failing here would make the fix for that state a
/// casualty of it too, leaving a hand-deleted cache directory as the only way out.
///
/// So a config, catalog, or resolution failure means "there is no previous bundle," and the run
/// reports against an empty one: every item reads as added, which is true, since nothing resolved
/// before. A network failure is passed on: it isn't about the cache being stale, and the
/// refreshing pass is about to hit the same network with a better error message.
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

/// Where one catalog's pin stands, from the two resolutions of it.
///
/// `moving` decides `pinned`, not the shape of the `ref` string: a hex-looking branch name is
/// legitimate, and the refresh already asked git which kind of ref answered.
fn pin_of(before: &Catalog, after: Option<&Catalog>) -> CatalogPin {
    let base = CatalogPin {
        name: before.name.clone(),
        source: before.source.clone(),
        r#ref: before.r#ref.clone(),
        freshness: CatalogFreshness::Unversioned,
        commit: None,
        latest: None,
    };

    // No commit on either side means a `path:` source: there is no revision, so nothing to be
    // behind of.
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

/// Where one catalog's pin stands when the cache it stood on did not resolve at all.
///
/// Read off the refreshing pass alone, the only one there is. `outdated` rather than `current` for
/// anything that can move: what the cache held is unusable and what the remote holds is not, and
/// the two commits can't be equal here since one of them resolves and the other doesn't.
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

    // No `commit`: the project resolves to nothing right now, so this row must not claim the commit
    // it failed at as the one it "resolves to".
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

/// Resolves the project twice and compares the two bundles.
///
/// The unrefreshed pass runs first; that ordering is load-bearing under `Advance`, since once the
/// cache's refs have moved, the question "what does this project resolve to now" has no answer
/// left.
///
/// It's also allowed to come back empty-handed (see [`resolve_before`]), in which case the report
/// is against a project that resolved to nothing (an empty [`Bundle`]) rather than a report that
/// never happened.
///
/// `mode` is `Probe` for a report that must change nothing, `Advance` to move the cache forward.
///
/// # Errors
///
/// Exit 2 for a malformed config, a catalog name the project does not configure, or an unreadable
/// catalog; exit 3 for a resolution error; exit 4 if a fetch fails. All of them from the refreshing
/// pass: the unrefreshed one fails only with what it cannot answer by refreshing.
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

/// `ambit outdated`: where every pin stands, and what moving it would bring.
///
/// Probes rather than fetches, so running it changes nothing about what a later `ambit install`
/// does. The cache is what makes a moving `ref:` deterministic between runs, so a read-only command
/// that quietly advanced it would move a pin nobody asked to move.
///
/// # Errors
///
/// Everything planning an update returns. Being outdated is never one of them; it is the report.
pub fn check_outdated(
    project_dir: &Path,
    env: &Env,
    options: &UpdateOptions,
) -> Result<UpdatePlan> {
    Ok(plan_update(project_dir, env, RefreshMode::Probe, options)?.plan)
}

/// `ambit update --dry-run`: the same report, restricted to the catalogs the run named.
///
/// Literally [`check_outdated`] with the options passed through. It can't go on to preview the
/// install, because the install it would preview runs against the pins as they stand (this run has
/// deliberately not moved them), so a plan of artifacts here would describe the wrong resolution.
/// What a dry run owes a reader is which pins would move and what the bundle would gain and lose,
/// which is exactly the report `outdated` produces.
///
/// # Errors
///
/// As [`check_outdated`].
pub fn preview_update(
    project_dir: &Path,
    env: &Env,
    options: &UpdateOptions,
) -> Result<UpdatePlan> {
    check_outdated(project_dir, env, options)
}

/// `ambit update`: move the pins, then install.
///
/// Installs via `install_project`, told one thing it doesn't otherwise know: which catalogs' lock
/// pins this run is replacing. Without that, it would reproduce the commits still written in the
/// lock and quietly undo the update. With it, those catalogs resolve from the cache whose refs this
/// run just advanced, so the install writes the commits that were reported without touching the
/// network.
///
/// `install` is `--adopt` and `--copy`/`--link`, passed through. Not `--frozen`: an update exists
/// to change the lock, so a flag that fails when the lock would change is a contradiction rather
/// than a combination, and it is not offered.
///
/// # Errors
///
/// Everything planning an update and `install_project` return.
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
