use std::cmp::Ordering;
use std::collections::HashSet;

use indexmap::{IndexMap, IndexSet};

use crate::errors::{AmbitError, ExitCode, Result, at, resolution_error};
use crate::model::catalog::{
    MergedCatalog, MergedHook, MergedMcp, MergedPack, MergedSkill, SKILL_FILENAME, qualified_name,
};
use crate::model::config::ProjectConfig;
use crate::model::expectation::{ExpectationSet, union_expectations};
use crate::model::pattern::{
    PatternEntry, PatternItem, REQUIRES_KEY, entry_yaml, format_entry, matches, unique_entries,
};
use crate::model::reference::Reference;
pub use crate::model::requirement::ItemKind;
use crate::model::requirement::KIND_SEPARATOR;
use crate::util::cmp::js_cmp;
use crate::util::string_enum;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub packs: Vec<MergedPack>,
    pub skills: Vec<MergedSkill>,
    pub mcps: Vec<MergedMcp>,
    pub hooks: Vec<MergedHook>,
}

pub type BundleItem = Reference<ItemKind>;

pub fn format_item(item: &BundleItem) -> String {
    format!("{}{KIND_SEPARATOR}{}", item.kind, item.name)
}

string_enum! {
    pub enum RequirerKind {
        Pack => "pack",
        Skill => "skill",
    }
}

impl RequirerKind {
    pub fn item_kind(self) -> ItemKind {
        match self {
            Self::Pack => ItemKind::Pack,
            Self::Skill => ItemKind::Skill,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirer {
    pub kind: RequirerKind,
    pub catalog: String,
    pub name: String,
    pub requires: Vec<PatternEntry>,
    pub file: String,
}

pub fn requirers_of(merged: &MergedCatalog) -> Vec<Requirer> {
    requirers_from(&merged.packs, &merged.skills)
}

fn requirers_from(packs: &[MergedPack], skills: &[MergedSkill]) -> Vec<Requirer> {
    packs
        .iter()
        .map(|pack| Requirer {
            kind: RequirerKind::Pack,
            catalog: pack.catalog.clone(),
            name: pack.name.clone(),
            requires: pack.requires.clone(),
            file: pack.file.clone(),
        })
        .chain(skills.iter().map(|skill| Requirer {
            kind: RequirerKind::Skill,
            catalog: skill.catalog.clone(),
            name: skill.name.clone(),
            requires: skill.requires.clone(),
            file: format!("{}/{SKILL_FILENAME}", skill.path),
        }))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionReason {
    Selected { entry: PatternEntry },
    RequiredBy { requirer: BundleItem },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReasonedItem {
    pub kind: ItemKind,
    pub name: String,
    pub reason: SelectionReason,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionReasons {
    pub packs: IndexMap<String, SelectionReason>,
    pub skills: IndexMap<String, SelectionReason>,
    pub mcps: IndexMap<String, SelectionReason>,
    pub hooks: IndexMap<String, SelectionReason>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bundle {
    pub packs: Vec<MergedPack>,
    pub skills: Vec<MergedSkill>,
    pub mcps: Vec<MergedMcp>,
    pub hooks: Vec<MergedHook>,
    pub expects: ExpectationSet,
    pub reasons: SelectionReasons,
}

fn pattern_item<'a>(kind: ItemKind, catalog: &'a str, name: &'a str) -> PatternItem<'a> {
    PatternItem {
        kind,
        catalog,
        name,
    }
}

pub fn selecting_entry<'e>(
    entries: &'e [PatternEntry],
    kind: ItemKind,
    catalog: &str,
    name: &str,
) -> Option<&'e PatternEntry> {
    let subject = pattern_item(kind, catalog, name);
    let mut best: Option<(&PatternEntry, String)> = None;

    for entry in entries.iter().filter(|entry| matches(entry, subject)) {
        let formatted = format_entry(entry);

        if best
            .as_ref()
            .is_none_or(|(_, seen)| js_cmp(&formatted, seen) == Ordering::Less)
        {
            best = Some((entry, formatted));
        }
    }

    best.map(|(entry, _)| entry)
}

pub fn matches_anything(entry: &PatternEntry, merged: &MergedCatalog) -> bool {
    merged.packs.iter().any(|pack| {
        matches(
            entry,
            pattern_item(ItemKind::Pack, &pack.catalog, &pack.name),
        )
    }) || merged.skills.iter().any(|skill| {
        matches(
            entry,
            pattern_item(ItemKind::Skill, &skill.catalog, &skill.name),
        )
    }) || merged
        .mcps
        .iter()
        .any(|mcp| matches(entry, pattern_item(ItemKind::Mcp, &mcp.catalog, &mcp.name)))
        || merged.hooks.iter().any(|hook| {
            matches(
                entry,
                pattern_item(ItemKind::Hook, &hook.catalog, &hook.name),
            )
        })
}

fn kind_label(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Pack => "pack",
        ItemKind::Skill => "skill",
        ItemKind::Mcp => "MCP server",
        ItemKind::Hook => "hook",
    }
}

pub fn unmatched_entry_error(
    entry: &PatternEntry,
    within: &str,
    r#where: &str,
    catalogs: &[String],
) -> AmbitError {
    let summary = format!(
        "`{REQUIRES_KEY}` entry \"{}\" matches nothing {where}",
        format_entry(entry)
    );

    if let Some(catalog) = entry.catalog.as_deref()
        && !catalogs.iter().any(|known| known == catalog)
    {
        let mut detail = vec![format!("no catalog in `catalogs:` is named \"{catalog}\"")];

        if catalog.contains('*') {
            detail.push(
                "a qualifier is an alias, not a pattern: `*` is matched literally there".to_owned(),
            );
        }

        detail.push(if catalogs.is_empty() {
            "this project configures no catalogs at all".to_owned()
        } else {
            format!("configured catalogs: {}", catalogs.join(", "))
        });
        detail.push("correct the qualifier, or add the catalog to `catalogs:`".to_owned());

        return resolution_error(summary, detail);
    }

    let mut detail = vec![format!(
        "no {} in catalog \"{within}\" has a name matching \"{}\"",
        kind_label(entry.kind),
        entry.pattern
    )];

    if entry.catalog.is_none() {
        detail.push(format!(
            "a catalog's own `{REQUIRES_KEY}` resolves within that catalog, which can only require what it ships"
        ));
    }

    detail.push("correct the pattern, add the item to a catalog, or remove the entry".to_owned());

    resolution_error(summary, detail)
}

pub fn entry_catalog(entry: &PatternEntry) -> &str {
    entry
        .catalog
        .as_deref()
        .expect("a project's `requires` entry is parsed as qualified")
}

pub fn assert_entries_match(config: &ProjectConfig, merged: &MergedCatalog) -> Result<()> {
    let mut entries: Vec<(String, &PatternEntry)> = config
        .requires
        .iter()
        .map(|entry| (format_entry(entry), entry))
        .collect();

    entries.sort_by(|(a, _), (b, _)| js_cmp(a, b));

    for (_, entry) in entries {
        if matches_anything(entry, merged) {
            continue;
        }

        return Err(unmatched_entry_error(
            entry,
            entry_catalog(entry),
            &entry_position(config, entry),
            &merged.catalogs,
        ));
    }

    Ok(())
}

pub fn entry_position(config: &ProjectConfig, entry: &PatternEntry) -> String {
    at(
        &config.origin.file,
        config.origin.entry_lines.get(&entry_yaml(entry)).copied(),
    )
}

pub fn requirer_position(requirer: &Requirer) -> String {
    at(&requirer.file, None)
}

pub fn required_entries(requirer: &Requirer) -> Vec<PatternEntry> {
    let mut entries: Vec<(String, PatternEntry)> = unique_entries(&requirer.requires)
        .into_iter()
        .map(|entry| (entry_yaml(&entry), entry))
        .collect();

    entries.sort_by(|(a, _), (b, _)| js_cmp(a, b));
    entries.into_iter().map(|(_, entry)| entry).collect()
}

pub fn required_items(
    entry: &PatternEntry,
    requirer: &Requirer,
    merged: &MergedCatalog,
) -> Selection {
    let own = |catalog: &str| catalog == requirer.catalog;

    Selection {
        packs: merged
            .packs
            .iter()
            .filter(|pack| {
                own(&pack.catalog)
                    && matches(
                        entry,
                        pattern_item(ItemKind::Pack, &pack.catalog, &pack.name),
                    )
            })
            .cloned()
            .collect(),
        skills: merged
            .skills
            .iter()
            .filter(|skill| {
                own(&skill.catalog)
                    && matches(
                        entry,
                        pattern_item(ItemKind::Skill, &skill.catalog, &skill.name),
                    )
            })
            .cloned()
            .collect(),
        mcps: merged
            .mcps
            .iter()
            .filter(|mcp| {
                own(&mcp.catalog)
                    && matches(entry, pattern_item(ItemKind::Mcp, &mcp.catalog, &mcp.name))
            })
            .cloned()
            .collect(),
        hooks: merged
            .hooks
            .iter()
            .filter(|hook| {
                own(&hook.catalog)
                    && matches(
                        entry,
                        pattern_item(ItemKind::Hook, &hook.catalog, &hook.name),
                    )
            })
            .cloned()
            .collect(),
    }
}

fn is_empty(selection: &Selection) -> bool {
    selection.packs.is_empty()
        && selection.skills.is_empty()
        && selection.mcps.is_empty()
        && selection.hooks.is_empty()
}

pub fn matches_own_catalog(
    entry: &PatternEntry,
    requirer: &Requirer,
    merged: &MergedCatalog,
) -> bool {
    !is_empty(&required_items(entry, requirer, merged))
}

pub fn cycle_error(cycle: &[BundleItem], requirer: &Requirer, entry: &PatternEntry) -> AmbitError {
    resolution_error(
        "requirement cycle",
        [
            cycle
                .iter()
                .map(format_item)
                .collect::<Vec<_>>()
                .join(" → "),
            format!("closed by `{}` in {}", format_entry(entry), requirer.file),
            format!("break the cycle by removing one `{REQUIRES_KEY}` entry"),
        ],
    )
}

fn requirer_key(requirer: &Requirer) -> String {
    node_key(requirer.kind, &requirer.catalog, &requirer.name)
}

fn node_key(kind: RequirerKind, catalog: &str, name: &str) -> String {
    format!("{kind}{KIND_SEPARATOR}{}", qualified_name(catalog, name))
}

fn next_requirers<'r>(
    required: &Selection,
    requirers: &'r IndexMap<String, Requirer>,
) -> Vec<&'r Requirer> {
    required
        .packs
        .iter()
        .map(|pack| node_key(RequirerKind::Pack, &pack.catalog, &pack.name))
        .chain(
            required
                .skills
                .iter()
                .map(|skill| node_key(RequirerKind::Skill, &skill.catalog, &skill.name)),
        )
        .filter_map(|key| requirers.get(&key))
        .collect()
}

struct Closure<'m> {
    merged: &'m MergedCatalog,
    chosen_packs: HashSet<String>,
    chosen_skills: HashSet<String>,
    chosen_mcps: HashSet<String>,
    chosen_hooks: HashSet<String>,
    requirers: IndexMap<String, Requirer>,
    path: Vec<Requirer>,
    closed: HashSet<String>,
}

impl Closure<'_> {
    fn follow(&mut self, requirer: &Requirer) -> Result<()> {
        let key = requirer_key(requirer);

        if self.closed.contains(&key) {
            return Ok(());
        }

        self.path.push(requirer.clone());
        let qualified = qualified_name(&requirer.catalog, &requirer.name);

        match requirer.kind {
            RequirerKind::Pack => self.chosen_packs.insert(qualified),
            RequirerKind::Skill => self.chosen_skills.insert(qualified),
        };

        for entry in required_entries(requirer) {
            let required = required_items(&entry, requirer, self.merged);

            if is_empty(&required) {
                return Err(unmatched_entry_error(
                    &entry,
                    &requirer.catalog,
                    &requirer_position(requirer),
                    &self.merged.catalogs,
                ));
            }

            for mcp in &required.mcps {
                self.chosen_mcps
                    .insert(qualified_name(&mcp.catalog, &mcp.name));
            }

            for hook in &required.hooks {
                self.chosen_hooks
                    .insert(qualified_name(&hook.catalog, &hook.name));
            }

            let next: Vec<Requirer> = next_requirers(&required, &self.requirers)
                .into_iter()
                .cloned()
                .collect();

            for child in next {
                let child_key = requirer_key(&child);
                let opened = self
                    .path
                    .iter()
                    .position(|seen| requirer_key(seen) == child_key);

                if let Some(opened) = opened {
                    let cycle: Vec<BundleItem> = self.path[opened..]
                        .iter()
                        .chain(std::iter::once(&child))
                        .map(|seen| BundleItem {
                            kind: seen.kind.item_kind(),
                            name: seen.name.clone(),
                        })
                        .collect();

                    return Err(cycle_error(&cycle, requirer, &entry));
                }

                self.follow(&child)?;
            }
        }

        self.path.pop();
        self.closed.insert(key);

        Ok(())
    }
}

pub fn close_over_requires(
    roots: &[Requirer],
    mcps: &[MergedMcp],
    hooks: &[MergedHook],
    merged: &MergedCatalog,
) -> Result<Selection> {
    let mut walk = Closure {
        merged,
        chosen_packs: HashSet::new(),
        chosen_skills: HashSet::new(),
        chosen_mcps: mcps
            .iter()
            .map(|mcp| qualified_name(&mcp.catalog, &mcp.name))
            .collect(),
        chosen_hooks: hooks
            .iter()
            .map(|hook| qualified_name(&hook.catalog, &hook.name))
            .collect(),
        requirers: requirers_of(merged)
            .into_iter()
            .map(|requirer| (requirer_key(&requirer), requirer))
            .collect(),
        path: Vec::new(),
        closed: HashSet::new(),
    };

    for root in roots {
        walk.follow(root)?;
    }

    Ok(Selection {
        packs: merged
            .packs
            .iter()
            .filter(|pack| {
                walk.chosen_packs
                    .contains(&qualified_name(&pack.catalog, &pack.name))
            })
            .cloned()
            .collect(),
        skills: merged
            .skills
            .iter()
            .filter(|skill| {
                walk.chosen_skills
                    .contains(&qualified_name(&skill.catalog, &skill.name))
            })
            .cloned()
            .collect(),
        mcps: merged
            .mcps
            .iter()
            .filter(|mcp| {
                walk.chosen_mcps
                    .contains(&qualified_name(&mcp.catalog, &mcp.name))
            })
            .cloned()
            .collect(),
        hooks: merged
            .hooks
            .iter()
            .filter(|hook| {
                walk.chosen_hooks
                    .contains(&qualified_name(&hook.catalog, &hook.name))
            })
            .cloned()
            .collect(),
    })
}

fn collision_error(kind: ItemKind, name: &str, catalogs: &[&str]) -> AmbitError {
    resolution_error(
        format!(
            "{} \"{name}\" is selected from more than one catalog",
            kind_label(kind)
        ),
        [
            format!("provided by: {}", catalogs.join(", ")),
            if kind == ItemKind::Pack {
                "a bundle holds one item per name, so there would be two answers to which one is installed"
                    .to_owned()
            } else {
                "a harness reads one entry per name, so both copies would be installed at the same path"
                    .to_owned()
            },
            format!(
                "select only one copy: narrow a `{REQUIRES_KEY}` pattern, or drop the entry that reaches the other catalog"
            ),
        ],
    )
}

fn assert_one_per_name<'a>(
    kind: ItemKind,
    items: impl Iterator<Item = (&'a str, &'a str)>,
) -> Result<()> {
    let mut providers: IndexMap<&str, Vec<&str>> = IndexMap::new();

    for (name, catalog) in items {
        providers.entry(name).or_default().push(catalog);
    }

    for (name, catalogs) in providers {
        if catalogs.len() > 1 {
            return Err(collision_error(kind, name, &catalogs));
        }
    }

    Ok(())
}

pub fn assert_no_collisions(selection: &Selection) -> Result<()> {
    assert_one_per_name(
        ItemKind::Pack,
        selection
            .packs
            .iter()
            .map(|item| (item.name.as_str(), item.catalog.as_str())),
    )?;
    assert_one_per_name(
        ItemKind::Skill,
        selection
            .skills
            .iter()
            .map(|item| (item.name.as_str(), item.catalog.as_str())),
    )?;
    assert_one_per_name(
        ItemKind::Mcp,
        selection
            .mcps
            .iter()
            .map(|item| (item.name.as_str(), item.catalog.as_str())),
    )?;
    assert_one_per_name(
        ItemKind::Hook,
        selection
            .hooks
            .iter()
            .map(|item| (item.name.as_str(), item.catalog.as_str())),
    )
}

pub fn format_reason(reason: &SelectionReason) -> String {
    match reason {
        SelectionReason::Selected { entry } => format_entry(entry),
        SelectionReason::RequiredBy { requirer } => {
            format!("required-by:{}", format_item(requirer))
        }
    }
}

fn unexplainable(item: &BundleItem, problem: String) -> AmbitError {
    AmbitError::new(
        ExitCode::Internal,
        format!("cannot explain {} \"{}\"", item.kind, item.name),
        [
            problem,
            "this is a bug in ambit; please report it".to_owned(),
        ],
    )
}

fn required_by_reason(item: PatternItem<'_>, selected: &[Requirer]) -> Option<SelectionReason> {
    selected
        .iter()
        .find(|candidate| {
            candidate.catalog == item.catalog
                && candidate.requires.iter().any(|entry| matches(entry, item))
        })
        .map(|requirer| SelectionReason::RequiredBy {
            requirer: BundleItem {
                kind: requirer.kind.item_kind(),
                name: requirer.name.clone(),
            },
        })
}

fn selection_reasons<'a>(
    items: impl Iterator<Item = (&'a str, &'a str)>,
    kind: ItemKind,
    entries: &[PatternEntry],
    selected: &[Requirer],
) -> Result<IndexMap<String, SelectionReason>> {
    let mut reasons = IndexMap::new();

    for (catalog, name) in items {
        let reason = match selecting_entry(entries, kind, catalog, name) {
            Some(entry) => Some(SelectionReason::Selected {
                entry: entry.clone(),
            }),
            None => required_by_reason(pattern_item(kind, catalog, name), selected),
        };

        let Some(reason) = reason else {
            return Err(unexplainable(
                &BundleItem {
                    kind,
                    name: name.to_owned(),
                },
                format!(
                    "it is in the bundle, but no `{REQUIRES_KEY}` entry and no `{REQUIRES_KEY}` edge selected it"
                ),
            ));
        };

        reasons.insert(name.to_owned(), reason);
    }

    Ok(reasons)
}

fn reasons_of(bundle: &Bundle, kind: ItemKind) -> &IndexMap<String, SelectionReason> {
    match kind {
        ItemKind::Pack => &bundle.reasons.packs,
        ItemKind::Skill => &bundle.reasons.skills,
        ItemKind::Mcp => &bundle.reasons.mcps,
        ItemKind::Hook => &bundle.reasons.hooks,
    }
}

pub fn is_selected(bundle: &Bundle, item: &BundleItem) -> bool {
    reasons_of(bundle, item.kind).contains_key(&item.name)
}

pub fn reason_of<'b>(bundle: &'b Bundle, item: &BundleItem) -> Result<&'b SelectionReason> {
    reasons_of(bundle, item.kind)
        .get(&item.name)
        .ok_or_else(|| unexplainable(item, "it is not in the bundle".to_owned()))
}

pub fn explain_selection(bundle: &Bundle, item: &BundleItem) -> Result<Vec<ReasonedItem>> {
    let mut chain: Vec<ReasonedItem> = Vec::new();
    let mut walked: IndexSet<String> = IndexSet::new();
    let mut current = item.clone();

    loop {
        let reason = reason_of(bundle, &current)?.clone();

        chain.push(ReasonedItem {
            kind: current.kind,
            name: current.name.clone(),
            reason: reason.clone(),
        });

        let SelectionReason::RequiredBy { requirer } = reason else {
            chain.reverse();

            return Ok(chain);
        };

        let next = format_item(&requirer);

        if walked.contains(&next) {
            return Err(unexplainable(
                &current,
                format!("the `requires` chain through {next} does not terminate"),
            ));
        }

        walked.insert(next);
        current = requirer;
    }
}

pub fn resolve_bundle(config: &ProjectConfig, merged: &MergedCatalog) -> Result<Bundle> {
    assert_entries_match(config, merged)?;
    let entries = &config.requires;
    let selects = |kind: ItemKind, catalog: &str, name: &str| {
        selecting_entry(entries, kind, catalog, name).is_some()
    };

    let roots: Vec<Requirer> = requirers_of(merged)
        .into_iter()
        .filter(|requirer| selects(requirer.kind.item_kind(), &requirer.catalog, &requirer.name))
        .collect();
    let mcps: Vec<MergedMcp> = merged
        .mcps
        .iter()
        .filter(|mcp| selects(ItemKind::Mcp, &mcp.catalog, &mcp.name))
        .cloned()
        .collect();
    let hooks: Vec<MergedHook> = merged
        .hooks
        .iter()
        .filter(|hook| selects(ItemKind::Hook, &hook.catalog, &hook.name))
        .cloned()
        .collect();
    let selection = close_over_requires(&roots, &mcps, &hooks, merged)?;

    // Must run before any map below keys on a bare name.
    assert_no_collisions(&selection)?;
    let Selection {
        packs,
        skills,
        mcps,
        hooks,
    } = selection;

    let selected_requirers = requirers_from(&packs, &skills);

    let expects = {
        let lists: Vec<&[_]> = skills
            .iter()
            .map(|skill| skill.expects.as_slice())
            .chain(mcps.iter().map(|mcp| mcp.expects.as_slice()))
            .chain(hooks.iter().map(|hook| hook.expects.as_slice()))
            .collect();

        union_expectations(&lists)
    };

    let reasons = SelectionReasons {
        packs: selection_reasons(
            packs
                .iter()
                .map(|item| (item.catalog.as_str(), item.name.as_str())),
            ItemKind::Pack,
            entries,
            &selected_requirers,
        )?,
        skills: selection_reasons(
            skills
                .iter()
                .map(|item| (item.catalog.as_str(), item.name.as_str())),
            ItemKind::Skill,
            entries,
            &selected_requirers,
        )?,
        mcps: selection_reasons(
            mcps.iter()
                .map(|item| (item.catalog.as_str(), item.name.as_str())),
            ItemKind::Mcp,
            entries,
            &selected_requirers,
        )?,
        hooks: selection_reasons(
            hooks
                .iter()
                .map(|item| (item.catalog.as_str(), item.name.as_str())),
            ItemKind::Hook,
            entries,
            &selected_requirers,
        )?,
    };

    Ok(Bundle {
        packs,
        skills,
        mcps,
        hooks,
        expects,
        reasons,
    })
}

#[cfg(test)]
mod tests;
