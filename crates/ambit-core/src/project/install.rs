//! Installation: resolve a project, then hand the bundle to each harness adapter.
//!
//! Order: load, resolve, plan, apply, write lock, write state, rewrite the gitignore block. The two
//! record-keeping writes (lock, state) come last: a crash mid-apply then leaves artifacts unowned
//! but present, which `doctor` can report, instead of state claiming files that were never written.
//! The lock records what *was* installed, so it must not claim a resolution that failed to
//! materialize.
//!
//! Everything up to the first write is [`plan_install`]. [`preview_install`] renders that plan for
//! `--dry-run` instead of applying it. `plan` is pure and testable; `apply` is the only thing that
//! touches disk. This split lets a dry run print the same plan rather than reimplement
//! installation, and lets `ambit prune` (`clean.rs`) reach the same bundle without materializing
//! it.
//!
//! `--frozen` is checked before anything is written, so a CI run with a stale committed lock leaves
//! the project untouched (planning and reading state are both reads).
//!
//! Every adapter plans before any of them applies, so ownership (`ownership.rs`) is checked against
//! the complete set of targets while the project is still untouched.
//!
//! Pruning runs after the last adapter and before the two record-keeping writes, so a failed prune
//! is retryable (state still owns what it was about to remove) and a failed `apply` leaves the
//! previous install standing.
//!
//! No value from the environment reaches an artifact. A `${VAR}` in an MCP entity becomes a
//! reference in the target harness's own syntax rather than a resolved value, so an adapter's
//! `plan` is a pure function of the bundle and the project; the environment is `doctor`'s concern,
//! not install's. The one thing read here is `HOME`, and only to decide [`install_scope`].

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
use crate::model::config::{ProjectConfig, load_project_config};
use crate::model::documents::DocumentShape;
use crate::model::git::RefreshMode;
use crate::model::sources::SourceContext;
use crate::model::state::{
    ArtifactMode, OwnedArtifact, STATE_VERSION, State, read_state, write_state,
};
use crate::project::gitignore::{GitignoreStatus, gitignore_status, write_gitignore_blocks};
use crate::project::lock::{
    assert_lock_current, build_lock, read_catalog_pins, read_lock_text, serialize_lock,
    write_lock_text,
};
use crate::project::ownership::{OwnershipOptions, authorize_plan};
use crate::project::prune::{PrunedArtifact, plan_prune, prune_artifacts};
use crate::resolution::resolve::{Bundle, resolve_bundle};
use crate::util::cmp::js_cmp;
use crate::util::env::Env;
use crate::util::json::{self, JsonValue};
use crate::util::path::normalize;

/// Every adapter this build ships, keyed by the name `harnesses` uses.
pub static ADAPTERS: LazyLock<IndexMap<&'static str, ProfileAdapter>> = LazyLock::new(|| {
    PROFILES
        .iter()
        .map(|&profile| (profile.name, adapter_for(profile)))
        .collect()
});

/// How an install was asked to behave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstallOptions {
    /// Fail rather than write when resolution would change the lock.
    pub frozen: bool,
    /// Resolve from the catalog cache alone, failing rather than fetching.
    pub offline: bool,
    /// Take ownership of existing unowned targets instead of refusing them.
    pub adopt: bool,
    /// `--copy` / `--link`: materialize every skill this way, whatever its source would have
    /// chosen. Absent means each skill follows its source, which is the mode to leave alone.
    pub mode: Option<ArtifactMode>,
}

/// One adapter and the artifacts it would write.
#[derive(Clone)]
pub struct AdapterPlan {
    pub adapter: &'static dyn HarnessAdapter,
    pub plan: Vec<PlannedArtifact>,
}

/// A project resolved and planned, with nothing written yet.
///
/// Every mutating command starts from this: `install` applies it, `--dry-run` prints it, and
/// `prune` uses it to know what the current bundle keeps. Sharing it keeps the three from
/// disagreeing about what the bundle is.
#[derive(Clone)]
pub struct PlannedInstall {
    pub bundle: Bundle,
    /// The harnesses planned for, deduplicated and sorted.
    pub harnesses: Vec<String>,
    /// Each adapter and its own plan, in harness order.
    pub plans: Vec<AdapterPlan>,
    /// Every adapter's plan flattened: what ownership and pruning are answered against.
    pub artifacts: Vec<PlannedArtifact>,
    /// Hooks a configured harness cannot express, in harness order. Reported, never fatal.
    pub skipped: Vec<SkippedHook>,
    /// What the last install recorded owning.
    pub prior: State,
    /// The lock as the bytes an install would write, which is what `--frozen` compares.
    pub lock_text: String,
}

/// What `install --dry-run` reports: everything the run would do, with the project untouched.
#[derive(Clone, Debug)]
pub struct InstallPreview {
    pub bundle: Bundle,
    pub harnesses: Vec<String>,
    /// What install would write.
    pub artifacts: Vec<PlannedArtifact>,
    /// What install would skip: a hook a configured harness cannot express.
    pub skipped: Vec<SkippedHook>,
    /// What install would remove, from state alone.
    pub pruned: Vec<PrunedArtifact>,
    /// Whether `ambit.lock` would change.
    pub lock_changed: bool,
    /// Whether each managed `.gitignore` block would change, one row per file.
    pub gitignore: Vec<GitignoreStatus>,
}

/// What an install did, for the command to report.
#[derive(Clone, Debug)]
pub struct InstallResult {
    pub bundle: Bundle,
    /// The harnesses written for, deduplicated and sorted.
    pub harnesses: Vec<String>,
    /// Everything now owned, in the order the adapters wrote it.
    pub artifacts: Vec<AppliedArtifact>,
    /// Hooks a configured harness could not express, and so was not given.
    pub skipped: Vec<SkippedHook>,
    /// What the previous install owned and this one does not, removed by path. No report prints
    /// it; the tests read it.
    #[cfg_attr(not(test), allow(dead_code))]
    pub pruned: Vec<PrunedArtifact>,
}

/// The platform's own answer for the home directory, used only when the passed environment has no
/// `HOME`.
fn platform_home() -> Option<PathBuf> {
    #[allow(deprecated)]
    std::env::home_dir()
}

/// Which config the harnesses will read this install as: `User` at the home directory, `Project`
/// anywhere else.
///
/// Every harness ambit writes for keeps its user-level config under the home directory, so
/// installing there is how a person gets one set of skills and hooks in every project at once. That
/// changes what a hook's `command` may say (see `hook_root`, `harness/definitions.rs`), and nothing
/// else: the same files, in the same places.
///
/// Detected from the root rather than declared in `ambit.yml`: a user-level install that did not
/// say so writes hooks resolving into whatever project is open, which fails silently and is
/// exploitable, so there is nothing here to opt into.
///
/// `HOME` is honoured over the platform's own answer, as `cache_root` (`model/git.rs`) does, so a
/// test can point a home directory somewhere disposable. `root` is the project root, absolute.
pub fn install_scope(root: &Path, env: &Env) -> InstallScope {
    let home = match env.get("HOME") {
        Some(home) => Some(PathBuf::from(home)),
        None => platform_home(),
    };

    match home {
        Some(home) if normalize(root) == normalize(&home) => InstallScope::User,
        _ => InstallScope::Project,
    }
}

/// Resolves configured harness names to adapters.
///
/// Shared with `status.rs`, which has to plan through exactly the adapters install would use, or
/// the two commands could disagree about whether a project is installed.
///
/// # Errors
///
/// Exit 2 for a harness this build has no adapter for: silently skipping it would leave a project
/// believing it was installed.
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

/// What makes two planned artifacts the same artifact.
///
/// A path, for anything owned as a path. For a config file the path is not enough, because ambit
/// owns *keys* there, not the file: two harnesses writing different entries into one document both
/// have to write. Identity there is the whole write: the section, the driver, the root keys it
/// seeds, and the entries themselves.
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

/// Every adapter's plan, with each artifact planned exactly once.
///
/// The skills directory is shared: every harness plans the same `.agents/skills/<name>` targets,
/// and two harnesses of one family plan the same skills link. A path is an artifact's identity, so
/// the first adapter to name one plans it and the rest defer. Without this, the second adapter's
/// `apply` finds a symlink the first just created and refuses, state records the same path twice,
/// and `install` prints it twice.
///
/// A config file two harnesses write the *same* entries into is deduped the same way: Claude and VS
/// Code read one `.claude/settings.json`, so a project configuring both writes it once. Anything
/// else differing about a config artifact (a different section, a different rendering of the same
/// hook) makes it a second write, since dropping it would install less than the project asked for.
///
/// A shared `.agents/hooks/<name>` dedupes the same way: every harness that can express a hook
/// plans the same directory for it.
///
/// `adapters` is the harnesses to plan for, sorted so the result does not depend on `ambit.yml`'s
/// spelling. This order decides who plans a shared target.
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

/// What the command doing the planning contributes, as against what the CLI parsed into
/// [`InstallOptions`].
///
/// Separate from the options because neither field is a flag anyone types. They are how `install`,
/// `install --dry-run`, `prune`, and `ambit update`'s trailing install say which of them is asking.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanContext {
    /// How a catalog with no pin to reproduce may consult its remote. Absent means not at all.
    ///
    /// See [`catalog_plan`] for why an unpinned catalog is the one that has to ask.
    pub refresh: Option<RefreshMode>,
    /// Catalogs whose recorded pin this run is deliberately moving past, by name.
    ///
    /// `ambit update`'s, and only `ambit update`'s. It has already advanced the shared clone's refs
    /// to the commits it just reported, and the lock on disk still holds the commits it is
    /// replacing, so honouring those pins would make the install undo the update it is part of.
    pub released: Vec<String>,
}

/// Which catalogs resolve to a recorded commit, and which are allowed to ask their remote.
struct CatalogPlan {
    pins: IndexMap<String, String>,
    refresh: Option<IndexMap<String, RefreshMode>>,
}

/// Decides, per catalog, whether it reproduces a commit or asks where its ref points.
///
/// A pinned catalog reproduces: `read_catalog_pins` hands back the commit the lock recorded for
/// every catalog whose `source` and `ref` still match config, and resolution takes that commit
/// instead of asking. This is what makes a committed lock mean something. Without it, a moving
/// `ref:` would be answered from the machine-wide cache, which refetches only when it cannot
/// resolve a ref at all, so the commit a project got would be whatever the shared clone held, and
/// any other project on the machine could move it.
///
/// An unpinned catalog asks. It is unpinned in three cases: no lock yet, a catalog added since the
/// lock was written, or a `ref:` just edited. None has an earlier resolution to reproduce, so `ref:
/// main` means the commit main names now. Inheriting the shared clone's answer instead would mean
/// an old catalog installs fine until it silently doesn't, surfacing later as a resolution error
/// about a catalog that has been correct upstream for weeks.
///
/// A `released` catalog does neither: its pin is dropped, and it is not refreshed, because `ambit
/// update` already moved the clone to the commit it reported, and asking again risks a different
/// answer.
///
/// `--offline` disables every refresh, but not pins: reproducing a recorded commit works offline
/// (the commit is in the cache), resolving a ref does not.
///
/// `doctor`, `clean`, and `prune` plan with no refresh mode: they report on or dismantle what is
/// installed rather than asking what catalogs say today. They still honour pins, so all three agree
/// with the install they describe.
///
/// `install` uses `Advance`, not `Probe`, for the same reason `ambit update` advances: the clone is
/// shared, so a probe would resolve against a commit the next run (reading the clone's own refs,
/// now with a lock) would disagree with. The cost, as the refresh plan in `update.rs` also notes,
/// is that another project pointed at the same repository sees the moved clone too.
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

/// Resolves the project and plans every adapter's writes, touching nothing.
///
/// Everything up to the first write lives here, so `install`, `install --dry-run`, and `ambit
/// prune` share one notion of what the project resolves to. Reading state is part of it: it is an
/// input to the run, not something the run decides, and reading is not touching the project.
///
/// `project_dir` is the project root, absolute; `env` is the command's environment (the cache
/// location, what a `git:` source authenticates with, and `HOME` for [`install_scope`]).
/// `options` contributes `--offline` and `--copy`/`--link`, the two that change a plan; `plan` says
/// which command is doing the planning (see [`PlanContext`] and [`catalog_plan`]).
///
/// # Errors
///
/// Exit 2 for a malformed config or catalog, an unknown harness, an unreadable state file, an
/// unreadable `ambit.lock`, or a locked commit the repository does not have; exit 3 for a
/// resolution error; exit 4 if a fetch fails, or under `--offline` when the cache cannot answer.
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

    // The environment arrives once, here: for source resolution (where the cache lives, what a
    // `git:` source authenticates with) and for `HOME`, which says whether this root is the user's
    // own. Nothing deeper reaches for ambient state of its own.
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

    // Serialized up front so `--frozen` compares the same bytes the run would go on to write,
    // rather than a second rendering that could differ.
    let lock = build_lock(&loaded, &bundle)?;
    let project = ProjectPaths {
        root: project_dir.to_path_buf(),
        scope: Some(install_scope(project_dir, env)),
        mode: options.mode,
    };

    // Every adapter plans before any of them writes, so the ownership check sees every target
    // before a project whose second skill collides is left with its first one already installed.
    let plans = plan_for(&adapters, &bundle, &project);
    let artifacts = plans
        .iter()
        .flat_map(|adapter_plan| adapter_plan.plan.iter().cloned())
        .collect();

    // Asked of every configured adapter, not only the ones that planned something, so a harness
    // with no hook mechanism is reported as skipping every hook rather than silently.
    let skipped = adapters
        .iter()
        .flat_map(|adapter| adapter.skips(&bundle))
        .collect();

    Ok(PlannedInstall {
        harnesses,
        plans,
        artifacts,
        skipped,
        prior: read_state(project_dir)?,
        lock_text: serialize_lock(&lock),
        bundle,
    })
}

/// What an install would do, without doing any of it: `install --dry-run`.
///
/// A print of the plan rather than a second implementation of installation: the artifacts come
/// from the same `plan` call `apply` would receive, the removals from the same `plan_prune` install
/// acts on, and the two derived files come from the same pure functions that write them, asked
/// whether they would change anything.
///
/// Ownership is checked, because a refusal is part of what would happen: a dry run of an install
/// that would stop should also stop, and say why. `--frozen` still refuses a stale lock, since
/// refusing is not a mutation.
///
/// # Errors
///
/// Everything [`install_project`] returns before its first write: exit 2 for a malformed config or
/// an unowned target, exit 3 for a resolution error, exit 4 for a fetch, exit 5 under `--frozen`.
pub fn preview_install(
    project_dir: &Path,
    env: &Env,
    options: InstallOptions,
) -> Result<InstallPreview> {
    // `Probe`, not `Advance`: an unpinned catalog resolves against what the remote says now (see
    // `catalog_plan`), and a preview must report that commit without moving the cache's own refs.
    let planned = plan_install(
        project_dir,
        env,
        options,
        &PlanContext {
            refresh: Some(RefreshMode::Probe),
            released: Vec::new(),
        },
    )?;

    if options.frozen {
        assert_lock_current(project_dir, &planned.lock_text)?;
    }

    authorize_plan(
        &planned.artifacts,
        &planned.prior,
        OwnershipOptions {
            adopt: options.adopt,
        },
    )?;

    let pruned = plan_prune(&planned.artifacts, &planned.prior)?;
    let owned: Vec<OwnedArtifact> = planned.artifacts.iter().map(OwnedArtifact::from).collect();
    let gitignore = gitignore_status(project_dir, &owned)?;
    let lock_changed = read_lock_text(project_dir)?.as_deref() != Some(planned.lock_text.as_str());

    Ok(InstallPreview {
        bundle: planned.bundle,
        harnesses: planned.harnesses,
        artifacts: planned.artifacts,
        skipped: planned.skipped,
        pruned,
        lock_changed,
        gitignore,
    })
}

/// Resolves the project and materializes the bundle.
///
/// `options` carries `--frozen`, `--offline`, `--adopt` and `--copy`/`--link`. `released` is the
/// catalogs whose recorded pin this install is moving past (`ambit update`'s; see
/// [`PlanContext::released`]). Empty for every other caller, which is what makes a plain `install`
/// reproduce the lock rather than move it.
///
/// # Errors
///
/// Exit 2 for a malformed config or catalog, an unknown harness, a target path or config key ambit
/// does not own and was not told to adopt, or a locked commit the repository does not have; exit 4
/// if a fetch fails, or under `--offline` when the cache cannot answer; exit 5 under `--frozen` when
/// the committed lock is not what resolution produces.
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

    if options.frozen {
        assert_lock_current(project_dir, &planned.lock_text)?;
    }

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

    // Against `prior`, not `owner`: what `--adopt` just took over is already in the plan, so the
    // two agree here, and pruning is answerable from what the last install recorded.
    let pruned = prune_artifacts(project_dir, &planned.artifacts, &planned.prior)?;

    write_lock_text(project_dir, &planned.lock_text)?;
    write_state(
        project_dir,
        &State {
            version: STATE_VERSION,
            harnesses: planned.harnesses.clone(),
            artifacts: artifacts.clone(),
        },
    )?;

    // Last, deliberately after state: the blocks are rendered afresh every run, so a failure here
    // costs nothing (the next install rewrites them), whereas failing before `write_state` would
    // leave correctly installed artifacts unowned and the next plain install refusing them.
    write_gitignore_blocks(project_dir, &artifacts)?;

    Ok(InstallResult {
        bundle: planned.bundle,
        harnesses: planned.harnesses,
        artifacts,
        skipped: planned.skipped,
        pruned,
    })
}

#[cfg(all(test, feature = "cli"))]
pub(crate) mod fixture;

#[cfg(all(test, feature = "cli"))]
mod tests;

#[cfg(all(test, feature = "cli"))]
mod harnesses_tests;
