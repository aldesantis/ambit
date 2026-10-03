//! Browsing a setup's catalogs, explaining selections, previewing rules and uninstalls.
//!
//! The session caches one [`LoadedSetup`]. Every export taking a draft text answers for that draft
//! (or the saved config when it is absent) and reuses the cache while the draft declares the same
//! catalogs; a draft with different catalogs is loaded again from the cache only, never the
//! network. Only [`SetupSession::load_catalogs`] with [`FetchPolicy::FetchMissing`] fetches.
//!
//! Nothing here runs catalog content: hook commands and MCP launch lines are returned as data.

use std::sync::Arc;

use ambit_core::errors::AmbitError;
use ambit_core::harness::adapter::{HookSkipReason, SkippedHook};
use ambit_core::model::config::{
    CONFIG_FILENAMES, ProjectConfig, find_config_file, load_project_config, parse_project_config,
};
use ambit_core::model::expectation::{Expectation, ExpectationKind};
use ambit_core::model::hook_entity;
use ambit_core::model::mcp_entity;
use ambit_core::model::pattern::{Addressing, PatternEntry, is_literal, parse_address};
use ambit_core::model::requirement::CATALOG_SEPARATOR;
use ambit_core::project::browse::{self as core, CatalogLoad, LoadedSetup};
use ambit_core::resolution::resolve::{
    BundleItem, ReasonedItem, SelectionReason as CoreReason, entry_catalog,
};
use ambit_core::resolution::routes::{self, Route, bundle_catalog, bundle_items, resolve_matched};
use ambit_core::util::control::Control;

use crate::control::{CancelToken, ProgressListener, control_of};
use crate::engine::SetupSession;
use crate::errors::{EngineError, guard};
use crate::git::git_env;
use crate::records::{ItemKind, ItemRef, SelectionEntry, SourceKind, source_kind};

/// The session's cached load.
#[derive(Default)]
struct BrowseState {
    loaded: Option<LoadedSetup>,
}

/// Whether loading catalogs may reach a remote.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchPolicy {
    /// Offline: a git catalog missing from the cache is reported as not cached.
    CacheOnly,
    /// Clone or fetch what the cache is missing. Cached refs are not checked for newer commits.
    FetchMissing,
}

/// How every configured catalog loaded, in config order.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CatalogsState {
    pub catalogs: Vec<CatalogAvailability>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CatalogAvailability {
    pub name: String,
    pub state: CatalogLoadState,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum CatalogLoadState {
    /// Browsable. `commit` is the revision loaded, absent for a local folder (`local`), which has
    /// none.
    Loaded { commit: Option<String>, local: bool },
    /// Loading was offline and the cache does not hold it: fetching would fix it.
    NotCached { error: EngineError },
    /// Anything else: a missing folder, an unknown ref, a malformed catalog, a failed fetch.
    Failed { error: EngineError },
}

/// Every item of every loaded catalog, and what went wrong resolving the selection.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct BrowseResult {
    pub catalogs: Vec<CatalogAvailability>,
    /// Packs, then skills, then MCP servers, then hooks, each by name, then catalog.
    pub items: Vec<BrowseItem>,
    /// Entries matching nothing, then the error that kept the rest from resolving (a cycle, one
    /// name from two catalogs), when there is one; then nothing is selected. Entries for catalogs
    /// that did not load are not judged: their catalog's state already says why.
    pub problems: Vec<EngineError>,
}

/// One catalog item, as the browser lists it.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct BrowseItem {
    pub kind: ItemKind,
    pub catalog: String,
    pub name: String,
    pub description: Option<String>,
    /// The item's own dependencies, unqualified: each resolves within `catalog`. Empty for MCP
    /// servers and hooks.
    pub requires: Vec<SelectionEntry>,
    /// Declared prerequisites. Empty for packs.
    pub prerequisites: Vec<Prerequisite>,
    /// Whether this catalog's copy is selected. Another catalog's copy of the name does not count.
    pub selected: bool,
    /// Every reason it is selected, entries first, then packs, then skills; empty when it is not.
    pub reasons: Vec<SelectionReason>,
    /// Configured agent tools that cannot install it. Only hooks have any.
    pub limitations: Vec<ToolLimitation>,
    pub detail: ItemDetail,
}

/// Something that must be true on the machine for an item to work.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum Prerequisite {
    EnvironmentVariable { name: String },
}

/// One reason an item is selected. An item can have several, and stays selected until every one
/// is gone.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum SelectionReason {
    /// A `requires` entry naming exactly this item.
    Direct { entry: SelectionEntry },
    /// A `requires` entry whose wildcard pattern matches it.
    Rule { entry: SelectionEntry },
    /// A selected pack includes it. `chain` runs from the entry that selects the root to this item.
    Pack {
        pack: ItemRef,
        chain: Vec<ChainLink>,
    },
    /// A selected skill depends on it. `chain` is as for `Pack`.
    Dependency {
        requirer: ItemRef,
        chain: Vec<ChainLink>,
    },
}

/// One step of a selection chain. The first link is selected by an entry, each later one by the
/// link before it.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ChainLink {
    pub item: ItemRef,
    pub reason: ChainReason,
}

#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum ChainReason {
    Entry { entry: SelectionEntry },
    RequiredBy { requirer: ItemRef },
}

/// An agent tool that skips an item, and why.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ToolLimitation {
    /// The agent tool id, as `harnesses` lists it.
    pub tool: String,
    pub reason: LimitationReason,
    /// The reason in a sentence, as the CLI words it.
    pub message: String,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitationReason {
    /// The tool has no hooks at all.
    NoHooks,
    /// The tool has no equivalent of the hook's event.
    NoEvent,
}

/// What one item's own file declares, beyond what every kind shares. Paths are catalog-relative.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum ItemDetail {
    /// `path` is the skill's folder; [`SetupSession::skill_document`] reads its `SKILL.md`.
    Skill { path: String },
    /// [`SetupSession::pack_contents`] lists what it brings.
    Pack { file: String },
    Mcp {
        file: String,
        transport: McpTransport,
    },
    Hook {
        /// The hook's folder.
        path: String,
        /// As spelled in `hook.yml`, such as `PreToolUse`.
        event: String,
        matcher: Option<String>,
        hook_type: HookType,
        /// As written, before an agent tool rewrites a script path. Never run.
        command: String,
        timeout_seconds: Option<i64>,
    },
}

/// How an MCP server is reached. Values are as written: `${VAR}` references are not expanded.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum McpTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<NamedValue>,
    },
    Http {
        url: String,
        bearer_token_env_var: Option<String>,
        headers: Vec<NamedValue>,
    },
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct NamedValue {
    pub name: String,
    pub value: String,
}

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookType {
    /// The command runs as written.
    Command,
    /// The command names a script the hook's folder ships.
    Script,
}

/// A skill's `SKILL.md`, split for read-only display.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SkillDocument {
    /// The frontmatter's YAML, without its `---` delimiters.
    pub frontmatter: String,
    /// The Markdown after the frontmatter, byte for byte.
    pub body: String,
    /// Catalog-relative.
    pub path: String,
}

/// What uninstalling one item takes.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct RemovalImpact {
    pub item: ItemRef,
    /// Every entry that selects the item on its own, each with the chain from it to the item. All
    /// of them must be removed or edited. Empty when the item is not selected.
    pub sustaining: Vec<SustainingEntry>,
    /// For each sustaining entry, what removing only that entry would uninstall.
    pub effects: Vec<EntryEffect>,
    /// What removing every sustaining entry would uninstall: the item and whatever only they kept.
    pub removed: Vec<ItemRef>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SustainingEntry {
    pub entry: SelectionEntry,
    pub chain: Vec<ChainLink>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct EntryEffect {
    pub entry: SelectionEntry,
    pub removes: Vec<ItemRef>,
}

/// A `requires` entry that matches nothing, with the error resolution would report.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct UnmatchedEntry {
    pub entry: SelectionEntry,
    pub error: EngineError,
}

// UniFFI hands every optional argument over owned; there is no borrowed `Option<&str>` form.
#[allow(clippy::needless_pass_by_value)]
#[uniffi::export]
impl SetupSession {
    /// Loads every catalog the draft (or the saved config) declares, one at a time, and caches
    /// the result for the other browse exports. Fetches authenticate with the engine's GitHub
    /// token. `listener` hears a [`Stage::LoadingCatalogs`](crate::records::Stage) report per
    /// catalog.
    ///
    /// # Errors
    ///
    /// [`EngineError::Config`] when the config is missing or does not parse, or `ambit.lock`
    /// cannot be read; [`EngineError::Canceled`] when `cancel` is set, which keeps the previous
    /// cache. A catalog that fails to load for any other reason is reported in the result.
    #[uniffi::method(default(cancel = None, listener = None))]
    pub fn load_catalogs(
        &self,
        draft_text: Option<String>,
        policy: FetchPolicy,
        cancel: Option<Arc<CancelToken>>,
        listener: Option<Arc<dyn ProgressListener>>,
    ) -> Result<CatalogsState, EngineError> {
        guard(|| {
            let config = self.browse_config(draft_text.as_deref())?;
            let loaded = self.load(config, policy, &control_of(cancel.as_ref(), listener))?;

            Ok(CatalogsState {
                catalogs: self.availability(&loaded),
            })
        })
    }

    /// Every item of every loaded catalog, selected or not, with why each selected one is.
    ///
    /// # Errors
    ///
    /// [`EngineError::Config`] when the config is missing or does not parse. Resolution problems
    /// are reported in the result.
    pub fn browse(&self, draft_text: Option<String>) -> Result<BrowseResult, EngineError> {
        guard(|| {
            let loaded = self.loaded_for(self.browse_config(draft_text.as_deref())?)?;
            let (items, problems) = core::browse(&loaded);

            Ok(BrowseResult {
                catalogs: self.availability(&loaded),
                items: items.into_iter().map(browse_item).collect(),
                problems: problems.iter().map(|error| self.error(error)).collect(),
            })
        })
    }

    /// Reads a skill's `SKILL.md` from the loaded catalogs.
    ///
    /// # Errors
    ///
    /// [`EngineError::Resolution`] when no loaded catalog holds the skill; [`EngineError::Config`]
    /// when its file cannot be read.
    pub fn skill_document(&self, catalog: &str, name: &str) -> Result<SkillDocument, EngineError> {
        guard(|| {
            let document = core::skill_document(&self.current()?, catalog, name)
                .map_err(|error| self.error(&error))?;

            Ok(SkillDocument {
                frontmatter: document.frontmatter,
                body: document.body,
                path: document.path,
            })
        })
    }

    /// What selecting one pack would install, the pack itself and its nested packs included.
    ///
    /// # Errors
    ///
    /// [`EngineError::Resolution`] when no loaded catalog holds the pack, or its contents do not
    /// resolve.
    pub fn pack_contents(&self, catalog: &str, name: &str) -> Result<Vec<ItemRef>, EngineError> {
        guard(|| {
            let bundle = core::pack_contents(&self.current()?, catalog, name)
                .map_err(|error| self.error(&error))?;

            Ok(bundle_items(&bundle)
                .into_iter()
                .map(|(item, catalog)| item_ref(&item, &catalog))
                .collect())
        })
    }

    /// The items a rule of `kind` with `pattern` in `catalog` matches right now, before any
    /// dependency is followed. Future catalog updates can change the answer.
    ///
    /// # Errors
    ///
    /// [`EngineError::Config`] when the pattern breaks the grammar, or the config does not parse;
    /// [`EngineError::Resolution`] when it matches nothing; the catalog's own load error when it is
    /// configured and did not load.
    pub fn preview_rule(
        &self,
        draft_text: Option<String>,
        catalog: &str,
        kind: ItemKind,
        pattern: &str,
    ) -> Result<Vec<ItemRef>, EngineError> {
        guard(|| {
            let address = format!("{catalog}{CATALOG_SEPARATOR}{pattern}");
            let entry = parse_address(kind.into(), &address, Addressing::Qualified)
                .map_err(EngineError::from)?;
            let loaded = self.loaded_for(self.browse_config(draft_text.as_deref())?)?;
            let found = core::rule_matches(&loaded, &entry).map_err(|error| self.error(&error))?;

            Ok(found.iter().map(|item| item_ref(item, catalog)).collect())
        })
    }

    /// The draft's (or saved config's) entries that match nothing in the loaded catalogs. Entries
    /// for catalogs that did not load are left out.
    ///
    /// # Errors
    ///
    /// [`EngineError::Config`] when the config is missing or does not parse.
    pub fn unmatched_entries(
        &self,
        draft_text: Option<String>,
    ) -> Result<Vec<UnmatchedEntry>, EngineError> {
        guard(|| {
            let loaded = self.loaded_for(self.browse_config(draft_text.as_deref())?)?;

            Ok(
                routes::unmatched_entries(&judged_config(&loaded), &loaded.merged)
                    .into_iter()
                    .map(|(entry, error)| UnmatchedEntry {
                        entry: SelectionEntry::from(&entry),
                        error: self.error(&error),
                    })
                    .collect(),
            )
        })
    }

    /// What uninstalling `item` would take under the draft (or the saved config).
    ///
    /// # Errors
    ///
    /// [`EngineError::Config`] when the config is missing or does not parse;
    /// [`EngineError::Resolution`] when the selection does not resolve.
    pub fn removal_impact(
        &self,
        draft_text: Option<String>,
        item: ItemRef,
    ) -> Result<RemovalImpact, EngineError> {
        guard(|| {
            let loaded = self.loaded_for(self.browse_config(draft_text.as_deref())?)?;
            let config = judged_config(&loaded);
            let error = |error: AmbitError| self.error(&error);
            let before = resolve_matched(&config, &loaded.merged).map_err(error)?;
            let subject = BundleItem {
                kind: item.kind.into(),
                name: item.name.clone(),
            };

            // Another catalog's copy of the name may be the selected one; this copy then has
            // nothing keeping it.
            if bundle_catalog(&before, &subject) != Some(item.catalog.as_str()) {
                return Ok(RemovalImpact {
                    item,
                    sustaining: Vec::new(),
                    effects: Vec::new(),
                    removed: Vec::new(),
                });
            }

            let impact =
                routes::removal_impact(&config, &loaded.merged, &subject).map_err(error)?;
            let located = |items: &[BundleItem]| -> Vec<ItemRef> {
                items
                    .iter()
                    .map(|found| {
                        item_ref(found, bundle_catalog(&before, found).unwrap_or_default())
                    })
                    .collect()
            };

            Ok(RemovalImpact {
                sustaining: impact
                    .sustaining
                    .iter()
                    .map(|sustained| SustainingEntry {
                        entry: SelectionEntry::from(&sustained.entry),
                        chain: chain(&sustained.chain, &item.catalog),
                    })
                    .collect(),
                effects: impact
                    .effects
                    .iter()
                    .map(|(entry, removes)| EntryEffect {
                        entry: SelectionEntry::from(entry),
                        removes: located(removes),
                    })
                    .collect(),
                removed: located(&impact.removed),
                item,
            })
        })
    }
}

impl SetupSession {
    fn error(&self, error: &AmbitError) -> EngineError {
        self.engine().error(error)
    }

    /// `draft_text` parsed under the name of the setup's config file, or the saved config.
    fn browse_config(&self, draft_text: Option<&str>) -> Result<ProjectConfig, EngineError> {
        let parsed = match draft_text {
            Some(text) => {
                let file = find_config_file(self.root_path())
                    .map_or_else(|_| CONFIG_FILENAMES[0].to_owned(), |found| found.file);

                parse_project_config(text, &file)
            }
            None => load_project_config(self.root_path()),
        };

        parsed.map_err(|error| self.error(&error))
    }

    /// Loads `config`'s catalogs and replaces the cache with the result.
    fn load(
        &self,
        config: ProjectConfig,
        policy: FetchPolicy,
        control: &Control,
    ) -> Result<LoadedSetup, EngineError> {
        let policy = match policy {
            FetchPolicy::CacheOnly => core::FetchPolicy::CacheOnly,
            FetchPolicy::FetchMissing => core::FetchPolicy::FetchMissing,
        };
        // Loading runs outside the state lock; see `with_state`.
        let env = git_env(self.engine());
        let loaded = core::load_setup(self.root_path(), &env, config, policy, control)
            .map_err(|error| self.error(&error))?;

        self.with_state(|state: &mut BrowseState| state.loaded = Some(loaded.clone()));
        Ok(loaded)
    }

    /// The cache answering for `config`, or a cache-only load of it when its catalogs differ.
    fn loaded_for(&self, config: ProjectConfig) -> Result<LoadedSetup, EngineError> {
        let reused = self.with_state(|state: &mut BrowseState| {
            state
                .loaded
                .as_ref()
                .and_then(|loaded| core::reconfigure(loaded, config.clone()))
        });

        match reused {
            Some(loaded) => {
                self.with_state(|state: &mut BrowseState| state.loaded = Some(loaded.clone()));
                Ok(loaded)
            }
            None => self.load(config, FetchPolicy::CacheOnly, &Control::default()),
        }
    }

    /// The cache, or the saved config loaded from the cache when there is none yet.
    fn current(&self) -> Result<LoadedSetup, EngineError> {
        match self.with_state(|state: &mut BrowseState| state.loaded.clone()) {
            Some(loaded) => Ok(loaded),
            None => self.load(
                self.browse_config(None)?,
                FetchPolicy::CacheOnly,
                &Control::default(),
            ),
        }
    }

    fn availability(&self, loaded: &LoadedSetup) -> Vec<CatalogAvailability> {
        loaded
            .catalogs
            .iter()
            .map(|load| CatalogAvailability {
                name: load.name().to_owned(),
                state: match load {
                    CatalogLoad::Loaded(catalog) => CatalogLoadState::Loaded {
                        commit: catalog.commit.clone(),
                        local: matches!(
                            source_kind(&catalog.source, catalog.r#ref.as_deref()),
                            Some(SourceKind::Local { .. })
                        ),
                    },
                    CatalogLoad::NotCached { error, .. } => CatalogLoadState::NotCached {
                        error: self.error(error),
                    },
                    CatalogLoad::Failed { error, .. } => CatalogLoadState::Failed {
                        error: self.error(error),
                    },
                },
            })
            .collect()
    }
}

/// The config's entries for catalogs that loaded, as [`core::browse`] judges them.
fn judged_config(loaded: &LoadedSetup) -> ProjectConfig {
    let is_loaded = |entry: &PatternEntry| {
        loaded.catalogs.iter().any(|load| {
            matches!(load, CatalogLoad::Loaded(catalog) if catalog.name == entry_catalog(entry))
        })
    };

    ProjectConfig {
        requires: loaded
            .config
            .requires
            .iter()
            .filter(|entry| is_loaded(entry))
            .cloned()
            .collect(),
        ..loaded.config.clone()
    }
}

fn item_ref(item: &BundleItem, catalog: &str) -> ItemRef {
    ItemRef {
        kind: item.kind.into(),
        catalog: catalog.to_owned(),
        name: item.name.clone(),
    }
}

/// A core chain with each link placed in `catalog`. A chain never leaves the catalog of the item
/// it explains: an item's own `requires` resolve within its catalog, and the root is selected by an
/// entry qualified with that catalog.
fn chain(links: &[ReasonedItem], catalog: &str) -> Vec<ChainLink> {
    links
        .iter()
        .map(|link| ChainLink {
            item: ItemRef {
                kind: link.kind.into(),
                catalog: catalog.to_owned(),
                name: link.name.clone(),
            },
            reason: match &link.reason {
                CoreReason::Selected { entry } => ChainReason::Entry {
                    entry: SelectionEntry::from(entry),
                },
                CoreReason::RequiredBy { requirer } => ChainReason::RequiredBy {
                    requirer: item_ref(requirer, catalog),
                },
            },
        })
        .collect()
}

fn reason(route: &Route, catalog: &str) -> SelectionReason {
    match route {
        Route::Entry { entry } if is_literal(&entry.pattern) => SelectionReason::Direct {
            entry: SelectionEntry::from(entry),
        },
        Route::Entry { entry } => SelectionReason::Rule {
            entry: SelectionEntry::from(entry),
        },
        Route::RequiredBy {
            requirer,
            chain: links,
        } => {
            let requirer_ref = item_ref(requirer, catalog);
            let links = chain(links, catalog);

            if requirer_ref.kind == ItemKind::Pack {
                SelectionReason::Pack {
                    pack: requirer_ref,
                    chain: links,
                }
            } else {
                SelectionReason::Dependency {
                    requirer: requirer_ref,
                    chain: links,
                }
            }
        }
    }
}

fn prerequisite(expectation: &Expectation) -> Prerequisite {
    match expectation.kind {
        ExpectationKind::Env => Prerequisite::EnvironmentVariable {
            name: expectation.name.clone(),
        },
    }
}

fn limitation(skipped: &SkippedHook) -> ToolLimitation {
    // Worded as `ambit install` warns about the same skip.
    let (reason, message) = match skipped.reason {
        HookSkipReason::NoMechanism => (
            LimitationReason::NoHooks,
            format!("{} has no declarative hook mechanism", skipped.harness),
        ),
        HookSkipReason::NoEvent => (
            LimitationReason::NoEvent,
            format!(
                "{} has no spelling for the {} event",
                skipped.harness, skipped.event
            ),
        ),
    };

    ToolLimitation {
        tool: skipped.harness.clone(),
        reason,
        message,
    }
}

fn named_values(values: impl IntoIterator<Item = (String, String)>) -> Vec<NamedValue> {
    values
        .into_iter()
        .map(|(name, value)| NamedValue { name, value })
        .collect()
}

fn detail(detail: core::ItemDetail) -> ItemDetail {
    match detail {
        core::ItemDetail::Skill { path } => ItemDetail::Skill { path },
        core::ItemDetail::Pack { file } => ItemDetail::Pack { file },
        core::ItemDetail::Mcp { file, transport } => ItemDetail::Mcp {
            file,
            transport: match transport {
                mcp_entity::McpTransport::Stdio(stdio) => McpTransport::Stdio {
                    command: stdio.command,
                    args: stdio.args,
                    env: named_values(stdio.env),
                },
                mcp_entity::McpTransport::Http(http) => McpTransport::Http {
                    url: http.url,
                    bearer_token_env_var: http.bearer_token_env_var,
                    headers: named_values(http.headers),
                },
            },
        },
        core::ItemDetail::Hook {
            path,
            event,
            matcher,
            hook_type,
            command,
            timeout,
        } => ItemDetail::Hook {
            path,
            event: event.as_str().to_owned(),
            matcher,
            hook_type: match hook_type {
                hook_entity::HookType::Command => HookType::Command,
                hook_entity::HookType::Script => HookType::Script,
            },
            command,
            timeout_seconds: timeout,
        },
    }
}

fn browse_item(item: core::BrowseItem) -> BrowseItem {
    BrowseItem {
        kind: item.kind.into(),
        reasons: item
            .routes
            .iter()
            .map(|route| reason(route, &item.catalog))
            .collect(),
        requires: item.requires.iter().map(SelectionEntry::from).collect(),
        prerequisites: item.expects.iter().map(prerequisite).collect(),
        limitations: item.limitations.iter().map(limitation).collect(),
        catalog: item.catalog,
        name: item.name,
        description: item.description,
        selected: item.selected,
        detail: detail(item.detail),
    }
}

#[cfg(test)]
mod tests;
