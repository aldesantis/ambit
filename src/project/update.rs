//! `ambit outdated` and `ambit update`: where each catalog's pin stands, and moving it.

use std::path::Path;

use crate::errors::Result;
use crate::model::state::ArtifactMode;
use crate::project::bundle_diff::BundleDiff;
use crate::project::install::InstallResult;
use crate::util::env::Env;
use crate::util::string_enum;

string_enum! {
    /// Where one catalog stands against its own `ref`.
    ///
    /// - `Outdated`: the ref names a different commit than the project resolves to now.
    /// - `Current`: it names the same one.
    /// - `Pinned`: the `ref` is a commit, so there is nothing for it to name differently.
    /// - `Unversioned`: a `path:` source, which has no revision at all.
    ///
    /// `Unversioned` is a separate word from `Current` because the two are not the same claim: a
    /// directory has no way to be *behind*, since there's no other revision to be behind of.
    pub enum CatalogFreshness {
        Outdated => "outdated",
        Current => "current",
        Pinned => "pinned",
        Unversioned => "unversioned",
    }
}

/// Every freshness, in declaration order.
pub const CATALOG_FRESHNESS: &[CatalogFreshness] = CatalogFreshness::ALL;

/// One catalog, and where its pin stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPin {
    pub name: String,
    /// The `source` as config wrote it.
    pub source: String,
    /// The `ref` as config wrote it, absent when the entry named none.
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

/// Whether any pin has somewhere to move.
pub fn has_outdated(plan: &UpdatePlan) -> bool {
    let _ = plan;
    todo!("port project/update.ts:hasOutdated")
}

/// `ambit outdated`: where every pin stands, and what moving it would bring.
///
/// Probes rather than fetches, so running it changes nothing about what a later `ambit install`
/// does.
///
/// # Errors
///
/// Everything planning an update returns. Being outdated is never one of them; it is the report.
pub fn check_outdated(
    project_dir: &Path,
    env: &Env,
    options: &UpdateOptions,
) -> Result<UpdatePlan> {
    let _ = (project_dir, env, options);
    todo!("port project/update.ts:checkOutdated")
}

/// `ambit update --dry-run`: the same report, restricted to the catalogs the run named.
///
/// Literally [`check_outdated`] with the options passed through.
///
/// # Errors
///
/// As [`check_outdated`].
pub fn preview_update(
    project_dir: &Path,
    env: &Env,
    options: &UpdateOptions,
) -> Result<UpdatePlan> {
    let _ = (project_dir, env, options);
    todo!("port project/update.ts:previewUpdate")
}

/// `ambit update`: move the pins, then install.
///
/// Installs via `install_project`, told which catalogs' lock pins this run is replacing. Not
/// `--frozen`: an update exists to change the lock.
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
    let _ = (project_dir, env, options, install);
    todo!("port project/update.ts:updateProject")
}
