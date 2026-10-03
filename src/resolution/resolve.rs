//! Resolution: the project's `requires` list and the merged catalog in, the bundle out.
//!
//! Pure and synchronous. All disk and network access happens before this runs, so the same inputs
//! always produce a byte-identical bundle and `resolve --json` can be committed as a golden file.
//!
//! A `requires` entry names a namespace and a glob matching names in it. An exact name is just a
//! pattern with no wildcard. The grammar and matcher live in `model/pattern.rs`; this file defines
//! what a match means.
//!
//! A pattern matching nothing is exit 3 ([`assert_entries_match`] for a project's entries,
//! [`close_over_requires`] for a catalog's own), via [`unmatched_entry_error`] in both cases. A
//! project's entry is qualified with the catalog it selects from; a catalog's own entry is bare and
//! resolves within that catalog only. Every selected item's reason is either the entry that
//! selected it or the requirer that pulled it in ([`SelectionReason`]), never both, and the lock
//! records it too.
//!
//! A **pack** and a **skill** both carry `requires`, and the closure follows both. A pack is a
//! document whose whole content is what asking for it gets you. A skill's `requires` declares what
//! it cannot work without, so a project that reaches it gets a working bundle rather than a broken
//! one. Servers and hooks are leaves.
//!
//! Two catalogs may provide one name; the merged catalog holds both copies, but a bundle holds at
//! most one: a selection reaching both is refused ([`assert_no_collisions`]), because harness
//! layout is flat and both copies would materialize to the same path. That refusal is what lets a
//! bare name serve as an identity within a bundle, while the merged catalog itself keys on
//! `<catalog>/<name>`.

use indexmap::IndexMap;

use crate::errors::{AmbitError, Result};
use crate::model::catalog::{MergedCatalog, MergedHook, MergedMcp, MergedPack, MergedSkill};
use crate::model::config::ProjectConfig;
use crate::model::expectation::ExpectationSet;
use crate::model::pattern::PatternEntry;
use crate::model::reference::Reference;
pub use crate::model::requirement::ItemKind;
use crate::util::string_enum;

/// A set of catalog items under consideration, each list in the merged catalog's own order (name,
/// then catalog).
///
/// Two copies of one name can be in here. [`close_over_requires`] produces a `Selection`;
/// [`assert_no_collisions`] judges it. A [`Bundle`] is a selection that has passed that check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub packs: Vec<MergedPack>,
    pub skills: Vec<MergedSkill>,
    pub mcps: Vec<MergedMcp>,
    pub hooks: Vec<MergedHook>,
}

/// One item of a bundle: which namespace, and the name inside it.
///
/// Not the same type a `requires` entry parses to: a pattern entry is a question about a catalog
/// answered by zero or more items, while a bundle item is exactly one item of one namespace.
pub type BundleItem = Reference<ItemKind>;

/// How a bundle item is written where only a string will do: `pack:engineering`.
pub fn format_item(item: &BundleItem) -> String {
    let _ = item;
    todo!("port resolution/resolve.ts:formatItem")
}

string_enum! {
    /// The two namespaces that can require anything.
    pub enum RequirerKind {
        Pack => "pack",
        Skill => "skill",
    }
}

impl RequirerKind {
    /// The same namespace as an [`ItemKind`].
    pub fn item_kind(self) -> ItemKind {
        match self {
            Self::Pack => ItemKind::Pack,
            Self::Skill => ItemKind::Skill,
        }
    }
}

/// An item that carries a `requires` list, as the closure and the cycle hunt see one.
///
/// A structural shape over the two kinds that have one (pack and skill), because everything below
/// asks the same four questions of both: which namespace, whose catalog, what name, and what does
/// it require. Which document to send a reader to is settled once, in [`requirers_of`].
///
/// A pack and a skill can share a name, so `kind` is part of a requirer's identity: `pack:core` and
/// `skill:core` are two nodes of the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirer {
    pub kind: RequirerKind,
    pub catalog: String,
    pub name: String,
    pub requires: Vec<PatternEntry>,
    /// The document its `requires` is written in, catalog-relative, so a refusal can name a file.
    ///
    /// A pack is its own document; a skill's annotations live in its `SKILL.md`.
    pub file: String,
}

/// Every item in the merged catalog that carries a `requires` list, packs first.
///
/// Packs first because a pack is the more useful answer to *what pulled this in*. The order is
/// otherwise the merged catalog's (name, then catalog), so "the first requirer that matches"
/// depends only on names.
pub fn requirers_of(merged: &MergedCatalog) -> Vec<Requirer> {
    let _ = merged;
    todo!("port resolution/resolve.ts:requirersOf")
}

/// Why one item is in the bundle: one of the two routes resolution offers, never both.
///
/// A `Selected` reason carries the entry itself rather than a rendering of it, so a caller can
/// print it whole. A `RequiredBy` reason names the requirer's namespace as well as its name,
/// because a pack may share a name with a skill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionReason {
    Selected { entry: PatternEntry },
    RequiredBy { requirer: BundleItem },
}

/// A bundle item with the reason it was selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReasonedItem {
    pub kind: ItemKind,
    pub name: String,
    pub reason: SelectionReason,
}

/// Every selected item's reason, keyed by name within each namespace.
///
/// Keyed by name, not address, because a bundle holds one item per name per namespace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionReasons {
    pub packs: IndexMap<String, SelectionReason>,
    pub skills: IndexMap<String, SelectionReason>,
    pub mcps: IndexMap<String, SelectionReason>,
    pub hooks: IndexMap<String, SelectionReason>,
}

/// The resolved set of packs, skills, MCP servers and hooks for a project.
///
/// One item per name within each namespace, guaranteed by [`assert_no_collisions`]. Everything
/// downstream (`install`, the lock, `status`, `doctor`, `why`) relies on that when it keys on a
/// bare name.
///
/// Packs are included even though a pack materializes nothing, so a bundle can answer *why is this
/// skill installed* with the pack name the project actually wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bundle {
    /// Selected packs, sorted by name. Materialized nowhere.
    pub packs: Vec<MergedPack>,
    /// Selected skills, sorted by name.
    pub skills: Vec<MergedSkill>,
    /// Selected MCP servers, sorted by name.
    pub mcps: Vec<MergedMcp>,
    /// Selected hooks, sorted by name.
    pub hooks: Vec<MergedHook>,
    /// Every precondition the selection declares, unioned and grouped by kind. Packs contribute
    /// nothing.
    pub expects: ExpectationSet,
    /// Why each of the above is here, one entry per selected item.
    pub reasons: SelectionReasons,
}

/// The entry that selected an item, or `None` when none did.
///
/// Several entries may reach one item; any of them is a true answer, so ties break on
/// [`format_entry`](crate::model::pattern::format_entry).
pub fn selecting_entry<'e>(
    entries: &'e [PatternEntry],
    kind: ItemKind,
    catalog: &str,
    name: &str,
) -> Option<&'e PatternEntry> {
    let _ = (entries, kind, catalog, name);
    todo!("port resolution/resolve.ts:selectingEntry")
}

/// Whether any item in any configured catalog is selected by `entry`.
pub fn matches_anything(entry: &PatternEntry, merged: &MergedCatalog) -> bool {
    let _ = (entry, merged);
    todo!("port resolution/resolve.ts:matchesAnything")
}

/// The error for a `requires` entry no item satisfies, used at every place a `requires` list is
/// written and checked.
///
/// A qualifier no catalog answers to is called out separately: the qualifier is an alias, not a
/// pattern.
///
/// `within` is the catalog the entry resolves in; `where` is the `(file line N)` suffix, as
/// [`at`](crate::errors::at) renders it; `catalogs` is every catalog the config listed, in config
/// order.
pub fn unmatched_entry_error(
    entry: &PatternEntry,
    within: &str,
    r#where: &str,
    catalogs: &[String],
) -> AmbitError {
    let _ = (entry, within, r#where, catalogs);
    todo!("port resolution/resolve.ts:unmatchedEntryError")
}

/// The catalog a project's `requires` entry selects from.
///
/// Non-optional, unlike [`PatternEntry::catalog`]: a project config is parsed as qualified, so an
/// entry naming no alias is refused at parse and cannot reach here.
pub fn entry_catalog(entry: &PatternEntry) -> &str {
    let _ = entry;
    todo!("port resolution/resolve.ts:entryCatalog")
}

/// Rejects a `requires` entry that selects nothing.
///
/// Stops at the first offender, sorted by `format_entry`, so which of several bad entries is
/// reported depends on what they say and not on config order.
///
/// # Errors
///
/// Exit 3, naming the entry and the config line it was written on.
pub fn assert_entries_match(config: &ProjectConfig, merged: &MergedCatalog) -> Result<()> {
    let _ = (config, merged);
    todo!("port resolution/resolve.ts:assertEntriesMatch")
}

/// Where an entry was written, as [`at`](crate::errors::at) renders it: the file alone if the
/// parse gave no line.
pub fn entry_position(config: &ProjectConfig, entry: &PatternEntry) -> String {
    let _ = (config, entry);
    todo!("port resolution/resolve.ts:entryPosition")
}

/// Where a requirer's `requires` entry was written: the file, and no line.
pub fn requirer_position(requirer: &Requirer) -> String {
    let _ = requirer;
    todo!("port resolution/resolve.ts:requirerPosition")
}

/// A requirer's `requires` list, deduplicated and sorted by `entry_yaml` rather than kept in
/// authoring order.
pub fn required_entries(requirer: &Requirer) -> Vec<PatternEntry> {
    let _ = requirer;
    todo!("port resolution/resolve.ts:requiredEntries")
}

/// Every item one of a requirer's `requires` entries selects, matched against the catalog that
/// requirer came from and against nothing else. Results stay in the merged catalog's order.
pub fn required_items(
    entry: &PatternEntry,
    requirer: &Requirer,
    merged: &MergedCatalog,
) -> Selection {
    let _ = (entry, requirer, merged);
    todo!("port resolution/resolve.ts:requiredItems")
}

/// Whether a requirer's `requires` entry selects anything its own catalog ships.
pub fn matches_own_catalog(
    entry: &PatternEntry,
    requirer: &Requirer,
    merged: &MergedCatalog,
) -> bool {
    let _ = (entry, requirer, merged);
    todo!("port resolution/resolve.ts:matchesOwnCatalog")
}

/// The error for a `requires` cycle: the whole path, and the edge that closed it.
///
/// `cycle` is the items around the loop, opening and closing on the same one; `requirer` is whose
/// `requires` closed the loop; `entry` is that entry.
pub fn cycle_error(cycle: &[BundleItem], requirer: &Requirer, entry: &PatternEntry) -> AmbitError {
    let _ = (cycle, requirer, entry);
    todo!("port resolution/resolve.ts:cycleError")
}

/// Closes a selection over `requires` until fixpoint: every item a selected pack or skill requires
/// joins the selection.
///
/// `roots` is what the project's entries selected among the two requiring kinds, in the merged
/// catalog's order; `mcps` and `hooks` are what they already selected of the leaves.
///
/// # Errors
///
/// Exit 3 for a `requires` entry that matches nothing in its own catalog, or a cycle.
pub fn close_over_requires(
    roots: &[Requirer],
    mcps: &[MergedMcp],
    hooks: &[MergedHook],
    merged: &MergedCatalog,
) -> Result<Selection> {
    let _ = (roots, mcps, hooks, merged);
    todo!("port resolution/resolve.ts:closeOverRequires")
}

/// Rejects a selection holding two catalogs' copies of one name.
///
/// # Errors
///
/// Exit 3, naming the item and every catalog that provides it.
pub fn assert_no_collisions(selection: &Selection) -> Result<()> {
    let _ = selection;
    todo!("port resolution/resolve.ts:assertNoCollisions")
}

/// How a reason reads in `--explain`, in the lock, and in `ambit why`.
pub fn format_reason(reason: &SelectionReason) -> String {
    let _ = reason;
    todo!("port resolution/resolve.ts:formatReason")
}

/// Whether an item is in the bundle.
pub fn is_selected(bundle: &Bundle, item: &BundleItem) -> bool {
    let _ = (bundle, item);
    todo!("port resolution/resolve.ts:isSelected")
}

/// Why one item of a bundle is in it.
///
/// # Errors
///
/// Exit 1 if the item is not in the bundle; check with [`is_selected`] first.
pub fn reason_of<'b>(bundle: &'b Bundle, item: &BundleItem) -> Result<&'b SelectionReason> {
    let _ = (bundle, item);
    todo!("port resolution/resolve.ts:reasonOf")
}

/// The whole chain behind one selected item, root cause first and the item itself last.
///
/// # Errors
///
/// Exit 1 if the item is not in the bundle, or the chain fails to terminate.
pub fn explain_selection(bundle: &Bundle, item: &BundleItem) -> Result<Vec<ReasonedItem>> {
    let _ = (bundle, item);
    todo!("port resolution/resolve.ts:explainSelection")
}

/// Computes the bundle for a project.
///
/// `expects` is unioned over the closed selection, not just the entry-selected one. Reasons are
/// computed here rather than on request, so `--explain`, `ambit why`, and the lock all report the
/// same answer.
///
/// # Errors
///
/// Exit 3 for a `requires` entry that matches nothing, a `requires` cycle, or one name selected
/// from two catalogs.
pub fn resolve_bundle(config: &ProjectConfig, merged: &MergedCatalog) -> Result<Bundle> {
    let _ = (config, merged);
    todo!("port resolution/resolve.ts:resolveBundle")
}
