//! Every route that keeps an item selected, and what removing one would take.
//!
//! [`resolve_bundle`] records one reason per item, the shortest true one, which is enough to answer
//! `ambit why`. Deciding whether an item can be uninstalled needs every route at once: an item
//! reached by a rule, a pack and a dependency stays installed until all three are gone. This module
//! answers that from the same matching the resolver uses, without changing what it selects.
//!
//! Closure distributes over union: the bundle of a `requires` list is the union of the bundles of
//! its entries taken one at a time. That is what lets [`removal_impact`] name exactly the entries an
//! item depends on by resolving each entry alone ([`entry_reach`]).
//!
//! A project entry that matches nothing reaches nothing. The functions here ignore such entries
//! rather than failing on them, so one bad rule in a draft does not hide every other route;
//! [`unmatched_entries`] reports them.
//!
//! Pure: nothing here reads the disk.

use crate::errors::{AmbitError, Result};
use crate::model::catalog::MergedCatalog;
use crate::model::config::ProjectConfig;
use crate::model::pattern::{PatternEntry, PatternItem, format_entry, matches, unique_entries};
use crate::resolution::resolve::{
    Bundle, BundleItem, ItemKind, ReasonedItem, SelectionReason, entry_catalog, entry_position,
    explain_selection, is_selected, matches_anything, resolve_bundle, unmatched_entry_error,
};
use crate::util::cmp::js_cmp;

/// One way an item is kept in the bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// A project entry selects the item. A literal pattern
    /// ([`is_literal`](crate::model::pattern::is_literal)) is a direct selection; any other is a
    /// rule.
    Entry { entry: PatternEntry },
    /// A selected pack or skill of the item's own catalog requires it: pack membership when the
    /// requirer is a pack, a dependency when it is a skill.
    ///
    /// `chain` runs from a project entry down to the item itself, following the bundle's recorded
    /// reasons ([`explain_selection`]), so its last element is the item with this route as reason.
    RequiredBy {
        requirer: BundleItem,
        chain: Vec<ReasonedItem>,
    },
}

/// Every route of one selected item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemRoutes {
    pub item: BundleItem,
    pub catalog: String,
    /// Entries first, ordered by [`format_entry`]; then requirers, packs before skills, each by
    /// name.
    pub routes: Vec<Route>,
}

/// One entry that keeps an item selected, and how it reaches the item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SustainingEntry {
    pub entry: PatternEntry,
    /// How the entry reaches the item: the entry first, the item last.
    pub chain: Vec<ReasonedItem>,
}

/// What uninstalling one item takes.
///
/// Nothing is removed implicitly: `sustaining` is the list of entries a caller must remove or edit,
/// and `removed` is what the bundle loses once all of them are gone, the item included.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalImpact {
    pub item: BundleItem,
    /// Every project entry whose own reach holds the item, ordered by [`format_entry`]. Removing
    /// any subset leaves the item installed; removing all of them uninstalls it.
    pub sustaining: Vec<SustainingEntry>,
    /// What each sustaining entry's removal alone would drop from the bundle, in `sustaining`
    /// order. Often empty, because another entry still reaches the same items.
    pub effects: Vec<(PatternEntry, Vec<BundleItem>)>,
    /// Everything the bundle loses when every sustaining entry is removed together.
    pub removed: Vec<BundleItem>,
}

/// One item of one namespace, in the shape the matcher takes.
fn pattern_item<'a>(kind: ItemKind, catalog: &'a str, name: &'a str) -> PatternItem<'a> {
    PatternItem {
        kind,
        catalog,
        name,
    }
}

/// `config` with its `requires` replaced.
fn with_requires(config: &ProjectConfig, requires: Vec<PatternEntry>) -> ProjectConfig {
    ProjectConfig {
        requires,
        ..config.clone()
    }
}

/// The config's entries that match something, deduplicated, in the order written.
fn matched_entries(config: &ProjectConfig, merged: &MergedCatalog) -> Vec<PatternEntry> {
    unique_entries(&config.requires)
        .into_iter()
        .filter(|entry| matches_anything(entry, merged))
        .collect()
}

/// `entries` sorted by [`format_entry`], the order every list of entries here is reported in.
fn sorted_entries(mut entries: Vec<PatternEntry>) -> Vec<PatternEntry> {
    entries.sort_by(|a, b| js_cmp(&format_entry(a), &format_entry(b)));
    entries
}

/// Resolves `config` without the entries that match nothing; see the module header.
///
/// # Errors
///
/// Exit 3 for a `requires` cycle or one name selected from two catalogs.
pub fn resolve_matched(config: &ProjectConfig, merged: &MergedCatalog) -> Result<Bundle> {
    resolve_bundle(
        &with_requires(config, matched_entries(config, merged)),
        merged,
    )
}

/// Every item of a bundle with its catalog, in report order: packs, skills, servers, hooks, each by
/// name.
pub fn bundle_items(bundle: &Bundle) -> Vec<(BundleItem, String)> {
    let item = |kind: ItemKind, name: &str, catalog: &str| {
        (
            BundleItem {
                kind,
                name: name.to_owned(),
            },
            catalog.to_owned(),
        )
    };

    bundle
        .packs
        .iter()
        .map(|pack| item(ItemKind::Pack, &pack.name, &pack.catalog))
        .chain(
            bundle
                .skills
                .iter()
                .map(|skill| item(ItemKind::Skill, &skill.name, &skill.catalog)),
        )
        .chain(
            bundle
                .mcps
                .iter()
                .map(|mcp| item(ItemKind::Mcp, &mcp.name, &mcp.catalog)),
        )
        .chain(
            bundle
                .hooks
                .iter()
                .map(|hook| item(ItemKind::Hook, &hook.name, &hook.catalog)),
        )
        .collect()
}

/// The catalog a selected item came from, or `None` when it is not in the bundle.
pub fn bundle_catalog<'b>(bundle: &'b Bundle, item: &BundleItem) -> Option<&'b str> {
    let found = match item.kind {
        ItemKind::Pack => bundle
            .packs
            .iter()
            .find(|pack| pack.name == item.name)
            .map(|pack| &pack.catalog),
        ItemKind::Skill => bundle
            .skills
            .iter()
            .find(|skill| skill.name == item.name)
            .map(|skill| &skill.catalog),
        ItemKind::Mcp => bundle
            .mcps
            .iter()
            .find(|mcp| mcp.name == item.name)
            .map(|mcp| &mcp.catalog),
        ItemKind::Hook => bundle
            .hooks
            .iter()
            .find(|hook| hook.name == item.name)
            .map(|hook| &hook.catalog),
    };

    found.map(String::as_str)
}

/// Every route of one item in `bundle`, as [`ItemRoutes::routes`] orders them.
fn routes_of(
    entries: &[PatternEntry],
    bundle: &Bundle,
    item: &BundleItem,
    catalog: &str,
) -> Result<Vec<Route>> {
    let subject = pattern_item(item.kind, catalog, &item.name);
    let mut routes: Vec<Route> = sorted_entries(
        entries
            .iter()
            .filter(|entry| matches(entry, subject))
            .cloned()
            .collect(),
    )
    .into_iter()
    .map(|entry| Route::Entry { entry })
    .collect();

    // The same test `required_by_reason` applies, keeping every match instead of the first. The
    // bundle's packs and skills are already sorted by name.
    let requirers = bundle
        .packs
        .iter()
        .map(|pack| (ItemKind::Pack, &pack.name, &pack.catalog, &pack.requires))
        .chain(bundle.skills.iter().map(|skill| {
            (
                ItemKind::Skill,
                &skill.name,
                &skill.catalog,
                &skill.requires,
            )
        }));

    for (kind, name, requirer_catalog, requires) in requirers {
        if requirer_catalog != catalog || !requires.iter().any(|entry| matches(entry, subject)) {
            continue;
        }

        let requirer = BundleItem {
            kind,
            name: name.clone(),
        };
        let mut chain = explain_selection(bundle, &requirer)?;

        chain.push(ReasonedItem {
            kind: item.kind,
            name: item.name.clone(),
            reason: SelectionReason::RequiredBy {
                requirer: requirer.clone(),
            },
        });
        routes.push(Route::RequiredBy { requirer, chain });
    }

    Ok(routes)
}

/// Every route of every item in `bundle`, in [`bundle_items`] order.
///
/// `bundle` is the resolution of `config` against `merged`, or of a draft config: nothing here
/// assumes it was saved.
///
/// # Errors
///
/// Exit 1 if the bundle cannot explain one of its own items, which is a resolver bug.
pub fn selection_routes(config: &ProjectConfig, bundle: &Bundle) -> Result<Vec<ItemRoutes>> {
    let entries = unique_entries(&config.requires);

    bundle_items(bundle)
        .into_iter()
        .map(|(item, catalog)| {
            Ok(ItemRoutes {
                routes: routes_of(&entries, bundle, &item, &catalog)?,
                item,
                catalog,
            })
        })
        .collect()
}

/// Every project entry that matches nothing, each with the error resolution would stop on.
///
/// Unlike [`assert_entries_match`](crate::resolution::resolve::assert_entries_match), which stops
/// at the first, this reports all of them, ordered by [`format_entry`] and deduplicated.
pub fn unmatched_entries(
    config: &ProjectConfig,
    merged: &MergedCatalog,
) -> Vec<(PatternEntry, AmbitError)> {
    sorted_entries(unique_entries(&config.requires))
        .into_iter()
        .filter(|entry| !matches_anything(entry, merged))
        .map(|entry| {
            let error = unmatched_entry_error(
                &entry,
                entry_catalog(&entry),
                &entry_position(config, &entry),
                &merged.catalogs,
            );

            (entry, error)
        })
        .collect()
}

/// The bundle `entry` resolves to on its own.
fn reach_bundle(
    config: &ProjectConfig,
    merged: &MergedCatalog,
    entry: &PatternEntry,
) -> Result<Bundle> {
    resolve_bundle(&with_requires(config, vec![entry.clone()]), merged)
}

/// Every item `entry` reaches on its own, closure included, in [`bundle_items`] order.
///
/// The entry need not be in `config`; only its catalogs are read.
///
/// # Errors
///
/// Exit 3 when the entry matches nothing, or its closure meets a cycle.
pub fn entry_reach(
    config: &ProjectConfig,
    merged: &MergedCatalog,
    entry: &PatternEntry,
) -> Result<Vec<BundleItem>> {
    Ok(bundle_items(&reach_bundle(config, merged, entry)?)
        .into_iter()
        .map(|(item, _)| item)
        .collect())
}

/// The items of `before` that `after` does not hold, in [`bundle_items`] order.
///
/// Removing entries can only shrink a bundle, so this is the whole difference.
fn dropped(before: &Bundle, after: &Bundle) -> Vec<BundleItem> {
    bundle_items(before)
        .into_iter()
        .map(|(item, _)| item)
        .filter(|item| !is_selected(after, item))
        .collect()
}

/// What uninstalling `item` takes: every entry keeping it selected, and what removing them drops.
///
/// An item not in the bundle has no sustaining entries and nothing to remove.
///
/// # Errors
///
/// Exit 3 when the config's matched entries do not resolve (a cycle, or one name from two
/// catalogs).
pub fn removal_impact(
    config: &ProjectConfig,
    merged: &MergedCatalog,
    item: &BundleItem,
) -> Result<RemovalImpact> {
    let entries = matched_entries(config, merged);
    let before = resolve_bundle(&with_requires(config, entries.clone()), merged)?;
    let mut sustaining = Vec::new();

    for entry in sorted_entries(entries.clone()) {
        let reach = reach_bundle(config, merged, &entry)?;

        if !is_selected(&reach, item) {
            continue;
        }

        sustaining.push(SustainingEntry {
            chain: explain_selection(&reach, item)?,
            entry,
        });
    }

    let without = |removed: &[&PatternEntry]| -> Result<Bundle> {
        let kept = entries
            .iter()
            .filter(|entry| !removed.contains(entry))
            .cloned()
            .collect();

        resolve_bundle(&with_requires(config, kept), merged)
    };

    let mut effects = Vec::new();

    for sustained in &sustaining {
        let after = without(&[&sustained.entry])?;

        effects.push((sustained.entry.clone(), dropped(&before, &after)));
    }

    let all: Vec<&PatternEntry> = sustaining
        .iter()
        .map(|sustained| &sustained.entry)
        .collect();
    let removed = if all.is_empty() {
        Vec::new()
    } else {
        dropped(&before, &without(&all)?)
    };

    Ok(RemovalImpact {
        item: item.clone(),
        sustaining,
        effects,
        removed,
    })
}

#[cfg(test)]
mod tests;
