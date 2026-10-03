//! Browsing a setup's catalogs: every item they offer, selected or not, with what selects it.
//!
//! Catalogs load one at a time ([`load_setup`]), so one unreachable or malformed catalog is
//! reported on its own and the others stay browsable. Everything after loading reads the parsed
//! catalogs and, for a skill's document, one file. Nothing here runs catalog content: a hook's
//! command and an MCP server's launch line are returned as data.
//!
//! The functions taking a [`LoadedSetup`] answer for `loaded.config`, which may be an unsaved
//! draft; [`reconfigure`] swaps the draft in without reloading when its catalogs are unchanged.

use std::path::Path;

use crate::errors::{AmbitError, ExitCode, Result, config_error, resolution_error};
use crate::harness::adapter::SkippedHook;
use crate::harness::definitions::PROFILES;
use crate::harness::profile::skipped_hooks;
use crate::model::catalog::{
    Catalog, CatalogLoadOptions, MergedCatalog, SKILL_FILENAME, load_catalogs, merge_catalogs,
};
use crate::model::config::ProjectConfig;
use crate::model::expectation::Expectation;
use crate::model::hook_entity::{HookEvent, HookType};
use crate::model::lock_file::read_catalog_pins;
use crate::model::mcp_entity::McpTransport;
use crate::model::pattern::{PatternEntry, PatternItem, matches};
use crate::model::requirement::{ITEM_KINDS, ItemKind};
use crate::model::sources::SourceContext;
use crate::model::yaml::split_frontmatter;
use crate::resolution::resolve::{
    Bundle, BundleItem, entry_catalog, entry_position, resolve_bundle, unmatched_entry_error,
};
use crate::resolution::routes::{
    Route, bundle_catalog, resolve_matched, selection_routes, unmatched_entries,
};
use crate::util::env::Env;
use crate::util::fs::read_text;
use crate::util::path::join;

/// How one configured catalog loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CatalogLoad {
    Loaded(Catalog),
    /// [`FetchPolicy::CacheOnly`] and the cache cannot answer: the exit-4 error a fetch would fix.
    NotCached {
        name: String,
        error: AmbitError,
    },
    /// Anything else: an unreadable source, an unknown ref, a malformed catalog, a failed fetch.
    Failed {
        name: String,
        error: AmbitError,
    },
}

impl CatalogLoad {
    /// The configured name of the catalog this load is for.
    pub fn name(&self) -> &str {
        match self {
            Self::Loaded(catalog) => &catalog.name,
            Self::NotCached { name, .. } | Self::Failed { name, .. } => name,
        }
    }
}

/// Whether loading may reach a remote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchPolicy {
    /// Offline: a git catalog the cache cannot answer is [`CatalogLoad::NotCached`].
    CacheOnly,
    /// Clone or fetch what the cache is missing, without checking cached refs for newer commits.
    FetchMissing,
}

/// A setup's config with its catalogs loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedSetup {
    pub config: ProjectConfig,
    /// One per configured catalog, in config order.
    pub catalogs: Vec<CatalogLoad>,
    /// The loaded catalogs merged. `merged.catalogs` names every configured catalog, loaded or not,
    /// so a message about an unknown qualifier lists what the config declares.
    pub merged: MergedCatalog,
}

/// Loads every catalog `config` declares, one at a time, each at the commit `ambit.lock` pins when
/// the lock still names the same source and ref.
///
/// # Errors
///
/// Exit 2 when `ambit.lock` exists and cannot be read. A catalog that fails to load is reported in
/// [`LoadedSetup::catalogs`], never as an error.
pub fn load_setup(
    root: &Path,
    env: &Env,
    config: ProjectConfig,
    policy: FetchPolicy,
) -> Result<LoadedSetup> {
    let pins = read_catalog_pins(root, &config)?;
    let context = SourceContext {
        project_dir: root.to_path_buf(),
        env: env.clone(),
        offline: policy == FetchPolicy::CacheOnly,
    };
    let mut catalogs = Vec::new();

    for entry in &config.catalogs {
        // `load_catalogs` stops at the first failure, so each catalog gets a load of its own.
        let single = ProjectConfig {
            catalogs: vec![entry.clone()],
            ..config.clone()
        };
        let mut options = CatalogLoadOptions {
            collect: None,
            refresh: None,
            pins: Some(pins.clone()),
        };
        let name = entry.name.clone();

        catalogs.push(match load_catalogs(&single, &context, &mut options) {
            Ok(mut loaded) => CatalogLoad::Loaded(loaded.remove(0)),
            Err(error) if policy == FetchPolicy::CacheOnly && error.code == ExitCode::Network => {
                CatalogLoad::NotCached { name, error }
            }
            Err(error) => CatalogLoad::Failed { name, error },
        });
    }

    let merged = merged_of(&config, &catalogs);

    Ok(LoadedSetup {
        config,
        catalogs,
        merged,
    })
}

/// The loaded catalogs merged, naming every configured catalog; see [`LoadedSetup::merged`].
fn merged_of(config: &ProjectConfig, catalogs: &[CatalogLoad]) -> MergedCatalog {
    let loaded: Vec<Catalog> = catalogs
        .iter()
        .filter_map(|load| match load {
            CatalogLoad::Loaded(catalog) => Some(catalog.clone()),
            CatalogLoad::NotCached { .. } | CatalogLoad::Failed { .. } => None,
        })
        .collect();

    MergedCatalog {
        catalogs: config
            .catalogs
            .iter()
            .map(|catalog| catalog.name.clone())
            .collect(),
        ..merge_catalogs(&loaded)
    }
}

/// `loaded` answering for `config` instead, or `None` when `config` declares different catalogs
/// (a name, source or ref changed) and must be loaded again.
pub fn reconfigure(loaded: &LoadedSetup, config: ProjectConfig) -> Option<LoadedSetup> {
    if loaded.config.catalogs != config.catalogs {
        return None;
    }

    Some(LoadedSetup {
        config,
        catalogs: loaded.catalogs.clone(),
        merged: loaded.merged.clone(),
    })
}

/// What one item's own document declares, beyond what every kind shares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemDetail {
    /// `path` is the skill's directory, catalog-relative; [`skill_document`] reads its `SKILL.md`.
    Skill { path: String },
    /// `file` is the pack's document, catalog-relative. Its resolved contents are
    /// [`pack_contents`].
    Pack { file: String },
    Mcp {
        file: String,
        transport: McpTransport,
    },
    Hook {
        /// The hook's directory, catalog-relative.
        path: String,
        event: HookEvent,
        matcher: Option<String>,
        hook_type: HookType,
        /// As written in `hook.yml`, before any harness rewrites a script path.
        command: String,
        /// Seconds.
        timeout: Option<i64>,
    },
}

/// One item of a loaded catalog, as a browser lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowseItem {
    pub kind: ItemKind,
    pub catalog: String,
    pub name: String,
    pub description: Option<String>,
    /// The item's own `requires`, unqualified: each resolves within `catalog`. Empty for servers
    /// and hooks.
    pub requires: Vec<PatternEntry>,
    /// Declared prerequisites. Empty for packs.
    pub expects: Vec<Expectation>,
    /// Whether this copy is in the bundle. Another catalog's copy of the same name does not count.
    pub selected: bool,
    /// Every route keeping it selected; empty when it is not.
    pub routes: Vec<Route>,
    /// Configured harnesses that cannot express it. Only hooks have any.
    pub limitations: Vec<SkippedHook>,
    pub detail: ItemDetail,
}

/// Whether a catalog of `loaded` parsed.
fn is_loaded(loaded: &LoadedSetup, catalog: &str) -> bool {
    loaded
        .catalogs
        .iter()
        .any(|load| matches!(load, CatalogLoad::Loaded(found) if found.name == catalog))
}

/// The config's entries for catalogs that loaded. An entry for a catalog that did not is not judged:
/// whether it matches is unknown, and the catalog's own load error already says why.
fn judged_config(loaded: &LoadedSetup) -> ProjectConfig {
    ProjectConfig {
        requires: loaded
            .config
            .requires
            .iter()
            .filter(|entry| is_loaded(loaded, entry_catalog(entry)))
            .cloned()
            .collect(),
        ..loaded.config.clone()
    }
}

/// The configured harnesses that would skip one hook, each with its reason.
fn hook_limitations(
    config: &ProjectConfig,
    merged: &MergedCatalog,
    index: usize,
) -> Vec<SkippedHook> {
    let hook = std::slice::from_ref(&merged.hooks[index]);

    PROFILES
        .iter()
        .filter(|profile| config.harnesses.iter().any(|name| name == profile.name))
        .flat_map(|profile| skipped_hooks(profile, hook))
        .collect()
}

/// Every item of every loaded catalog, packs then skills then servers then hooks, each in the
/// merged catalog's order (name, then catalog), plus every problem resolving the selection met.
///
/// The problems are the entries matching nothing ([`unmatched_entries`]) and, when the rest does
/// not resolve (a cycle, or one name from two catalogs), that error; then nothing is selected.
pub fn browse(loaded: &LoadedSetup) -> (Vec<BrowseItem>, Vec<AmbitError>) {
    let config = judged_config(loaded);
    let merged = &loaded.merged;
    let mut problems: Vec<AmbitError> = unmatched_entries(&config, merged)
        .into_iter()
        .map(|(_, error)| error)
        .collect();

    let bundle = resolve_matched(&config, merged).unwrap_or_else(|error| {
        problems.push(error);
        Bundle::default()
    });
    let routes = selection_routes(&config, &bundle).unwrap_or_else(|error| {
        problems.push(error);
        Vec::new()
    });

    let item = |kind: ItemKind,
                catalog: &str,
                name: &str,
                description: Option<&String>,
                requires: &[PatternEntry],
                expects: &[Expectation],
                detail: ItemDetail| {
        let reference = BundleItem {
            kind,
            name: name.to_owned(),
        };
        let selected = bundle_catalog(&bundle, &reference) == Some(catalog);
        let item_routes = if selected {
            routes
                .iter()
                .find(|found| found.item == reference)
                .map(|found| found.routes.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        BrowseItem {
            kind,
            catalog: catalog.to_owned(),
            name: name.to_owned(),
            description: description.cloned(),
            requires: requires.to_vec(),
            expects: expects.to_vec(),
            selected,
            routes: item_routes,
            limitations: Vec::new(),
            detail,
        }
    };

    let mut items = Vec::new();

    for kind in ITEM_KINDS {
        match kind {
            ItemKind::Pack => items.extend(merged.packs.iter().map(|pack| {
                item(
                    ItemKind::Pack,
                    &pack.catalog,
                    &pack.name,
                    pack.description.as_ref(),
                    &pack.requires,
                    &[],
                    ItemDetail::Pack {
                        file: pack.file.clone(),
                    },
                )
            })),
            ItemKind::Skill => items.extend(merged.skills.iter().map(|skill| {
                item(
                    ItemKind::Skill,
                    &skill.catalog,
                    &skill.name,
                    skill.description.as_ref(),
                    &skill.requires,
                    &skill.expects,
                    ItemDetail::Skill {
                        path: skill.path.clone(),
                    },
                )
            })),
            ItemKind::Mcp => items.extend(merged.mcps.iter().map(|mcp| {
                item(
                    ItemKind::Mcp,
                    &mcp.catalog,
                    &mcp.name,
                    None,
                    &[],
                    &mcp.expects,
                    ItemDetail::Mcp {
                        file: mcp.file.clone(),
                        transport: mcp.transport.clone(),
                    },
                )
            })),
            ItemKind::Hook => {
                items.extend(
                    merged
                        .hooks
                        .iter()
                        .enumerate()
                        .map(|(index, hook)| BrowseItem {
                            limitations: hook_limitations(&config, merged, index),
                            ..item(
                                ItemKind::Hook,
                                &hook.catalog,
                                &hook.name,
                                hook.description.as_ref(),
                                &[],
                                &hook.expects,
                                ItemDetail::Hook {
                                    path: hook.path.clone(),
                                    event: hook.event,
                                    matcher: hook.matcher.clone(),
                                    hook_type: hook.r#type,
                                    command: hook.command.clone(),
                                    timeout: hook.timeout,
                                },
                            )
                        }),
                );
            }
        }
    }

    (items, problems)
}

/// A skill's `SKILL.md`, split for read-only display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillDocument {
    /// The frontmatter block's YAML, without its delimiters.
    pub frontmatter: String,
    /// The Markdown after the closing delimiter, byte for byte.
    pub body: String,
    /// The document, catalog-relative.
    pub path: String,
}

/// The error for an item `loaded` does not hold.
fn unknown_item(kind: &str, catalog: &str, name: &str) -> AmbitError {
    resolution_error(
        format!("no {kind} \"{name}\" in catalog \"{catalog}\""),
        ["it may have been removed from the catalog; load the catalogs again"],
    )
}

/// The Markdown after a document's frontmatter: `close` minus the block's trailing blank lines and
/// the closing delimiter line.
fn body_of(close: &str) -> &str {
    let Some(delimiter) = close.find("\n---") else {
        return "";
    };
    let after = &close[delimiter + 1..];

    after.find('\n').map_or("", |newline| &after[newline + 1..])
}

/// Reads one skill's `SKILL.md` from its loaded catalog.
///
/// # Errors
///
/// Exit 3 when the catalog holds no such skill; exit 2 when the file cannot be read or its
/// frontmatter cannot be located.
pub fn skill_document(loaded: &LoadedSetup, catalog: &str, name: &str) -> Result<SkillDocument> {
    let skill = loaded
        .merged
        .skills
        .iter()
        .find(|skill| skill.catalog == catalog && skill.name == name)
        .ok_or_else(|| unknown_item("skill", catalog, name))?;
    let path = format!("{}/{SKILL_FILENAME}", skill.path);
    let absolute = join(&skill.catalog_root, &path);
    let text = read_text(&absolute).map_err(|error| {
        config_error(
            format!("cannot read {path} in catalog \"{catalog}\""),
            [
                format!("{error}: {}", absolute.display()),
                "load the catalogs again".to_owned(),
            ],
        )
    })?;
    let split = split_frontmatter(&text, &path)?;

    Ok(SkillDocument {
        frontmatter: split.block,
        body: body_of(&split.close).to_owned(),
        path,
    })
}

/// The entry selecting exactly one item.
fn exact_entry(kind: ItemKind, catalog: &str, name: &str) -> PatternEntry {
    PatternEntry {
        kind,
        pattern: name.to_owned(),
        catalog: Some(catalog.to_owned()),
    }
}

/// What selecting one pack would bring, as if it were the setup's only entry.
///
/// # Errors
///
/// Exit 3 when the catalog holds no such pack, or its closure meets an unmatched entry or a cycle.
pub fn pack_contents(loaded: &LoadedSetup, catalog: &str, name: &str) -> Result<Bundle> {
    if !loaded
        .merged
        .packs
        .iter()
        .any(|pack| pack.catalog == catalog && pack.name == name)
    {
        return Err(unknown_item("pack", catalog, name));
    }

    let config = ProjectConfig {
        requires: vec![exact_entry(ItemKind::Pack, catalog, name)],
        ..loaded.config.clone()
    };

    resolve_bundle(&config, &loaded.merged)
}

/// Every item `entry` matches by itself right now, without the closure: what a rule previews.
///
/// The answer can change whenever the catalog does. In the merged catalog's order.
///
/// # Errors
///
/// The catalog's own load error when it is configured and did not load; exit 3 when the entry
/// matches nothing, with the message resolution would print.
pub fn rule_matches(loaded: &LoadedSetup, entry: &PatternEntry) -> Result<Vec<BundleItem>> {
    let catalog = entry_catalog(entry);

    if let Some(load) = loaded.catalogs.iter().find(|load| load.name() == catalog) {
        match load {
            CatalogLoad::Loaded(_) => {}
            CatalogLoad::NotCached { error, .. } | CatalogLoad::Failed { error, .. } => {
                return Err(error.clone());
            }
        }
    }

    let merged = &loaded.merged;
    let names: Vec<(&str, &str)> = match entry.kind {
        ItemKind::Pack => merged
            .packs
            .iter()
            .map(|item| (item.catalog.as_str(), item.name.as_str()))
            .collect(),
        ItemKind::Skill => merged
            .skills
            .iter()
            .map(|item| (item.catalog.as_str(), item.name.as_str()))
            .collect(),
        ItemKind::Mcp => merged
            .mcps
            .iter()
            .map(|item| (item.catalog.as_str(), item.name.as_str()))
            .collect(),
        ItemKind::Hook => merged
            .hooks
            .iter()
            .map(|item| (item.catalog.as_str(), item.name.as_str()))
            .collect(),
    };
    let found: Vec<BundleItem> = names
        .into_iter()
        .filter(|(item_catalog, name)| {
            matches(
                entry,
                PatternItem {
                    kind: entry.kind,
                    catalog: item_catalog,
                    name,
                },
            )
        })
        .map(|(_, name)| BundleItem {
            kind: entry.kind,
            name: name.to_owned(),
        })
        .collect();

    if found.is_empty() {
        return Err(unmatched_entry_error(
            entry,
            catalog,
            &entry_position(&loaded.config, entry),
            &merged.catalogs,
        ));
    }

    Ok(found)
}

#[cfg(test)]
mod tests;
