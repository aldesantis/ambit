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
//! document whose whole content is what asking for it gets you: a catalog's way of offering a
//! named, browsable group of items. A skill's `requires` declares what it cannot work without, so a
//! project that reaches it gets a working bundle rather than a broken one. Servers and hooks are
//! leaves.
//!
//! Two catalogs may provide one name; the merged catalog holds both copies, but a bundle holds at
//! most one: a selection reaching both is refused ([`assert_no_collisions`]), because harness
//! layout is flat and both copies would materialize to the same path. That refusal is what lets a
//! bare name serve as an identity within a bundle, while the merged catalog itself keys on
//! `<catalog>/<name>`.

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
/// A [`Reference`] over the item kinds. Not the same type a `requires` entry parses to: a pattern
/// entry is a question about a catalog answered by zero or more items, while a bundle item is
/// exactly one item of one namespace.
pub type BundleItem = Reference<ItemKind>;

/// How a bundle item is written where only a string will do: `pack:engineering`.
pub fn format_item(item: &BundleItem) -> String {
    format!("{}{KIND_SEPARATOR}{}", item.kind, item.name)
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
/// A structural shape over the two kinds that have one (pack and skill), rather than an enum of
/// the two merged types, because everything below asks the same four questions of both: which
/// namespace, whose catalog, what name, and what does it require. Which document to send a reader
/// to is settled once, in [`requirers_of`].
///
/// A pack and a skill can share a name, so `kind` is part of a requirer's identity: `pack:core` and
/// `skill:core` are two nodes of the graph, and a cycle through one is not a cycle through the
/// other.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirer {
    /// Which namespace it is in: the two that can require anything.
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
/// Packs first because a pack is the more useful answer to *what pulled this in*: it is a name a
/// project wrote on purpose, where a skill's own requirement is an implementation detail of that
/// skill. The order is otherwise the merged catalog's (name, then catalog), so "the first requirer
/// that matches" depends only on names.
pub fn requirers_of(merged: &MergedCatalog) -> Vec<Requirer> {
    requirers_from(&merged.packs, &merged.skills)
}

/// [`requirers_of`] over a pack list and a skill list that need not be the whole merged catalog.
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

/// Why one item is in the bundle: one of the two routes resolution offers, never both.
///
/// A `Selected` reason carries the entry itself rather than a rendering of it, so a caller can
/// print it whole ([`format_entry`]).
///
/// A `RequiredBy` reason names the requirer's namespace as well as its name, because a pack may
/// share a name with a skill: `required-by:pack:engineering` is unambiguous,
/// `required-by:engineering` is not.
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
/// Keyed by name, not address, because a bundle holds one item per name per namespace:
/// [`assert_no_collisions`] refuses anything else.
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
/// bare name; the merged catalog itself does not, since it holds every catalog's copy.
///
/// Packs are included even though a pack materializes nothing. Everything else in a bundle lands
/// where a harness reads it, while a pack only contributes to the other three lists. It is kept so
/// a bundle can answer *why is this skill installed* with the pack name the project actually wrote.
///
/// The config's own `requires` list is not echoed back: it is already in the file the reader has
/// open. What they cannot look up is [`Bundle::reasons`], one per selected item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bundle {
    /// Selected packs, sorted by name. Materialized nowhere; see above.
    pub packs: Vec<MergedPack>,
    /// Selected skills, sorted by name.
    pub skills: Vec<MergedSkill>,
    /// Selected MCP servers, sorted by name.
    pub mcps: Vec<MergedMcp>,
    /// Selected hooks, sorted by name.
    pub hooks: Vec<MergedHook>,
    /// Every precondition the selection declares, unioned and grouped by kind.
    ///
    /// Grouped rather than flat because an expectation's kind decides what checking it means: a
    /// variable is looked up in the environment, a `bin:` on the `PATH`. Packs contribute nothing;
    /// a pack reads nothing from the world, and the items it names carry their own expectations.
    pub expects: ExpectationSet,
    /// Why each of the above is here, one entry per selected item.
    pub reasons: SelectionReasons,
}

/// One item of one namespace, in the shape the matcher takes.
fn pattern_item<'a>(kind: ItemKind, catalog: &'a str, name: &'a str) -> PatternItem<'a> {
    PatternItem {
        kind,
        catalog,
        name,
    }
}

/// The entry that selected an item, or `None` when none did.
///
/// Several entries may reach one item (a wildcard and the exact name under it); any of them is a
/// true answer, so ties break on [`format_entry`]: the same string the reason prints, so a tie
/// between two entries that print identically is unobservable.
pub fn selecting_entry<'e>(
    entries: &'e [PatternEntry],
    kind: ItemKind,
    catalog: &str,
    name: &str,
) -> Option<&'e PatternEntry> {
    let subject = pattern_item(kind, catalog, name);
    let mut best: Option<(&PatternEntry, String)> = None;

    // The first of the smallest, as a stable sort followed by `[0]` would pick.
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

/// Whether any item in any configured catalog is selected by `entry`.
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

/// What a namespace is called in a message about one of its members, without an article.
fn kind_label(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Pack => "pack",
        ItemKind::Skill => "skill",
        ItemKind::Mcp => "MCP server",
        ItemKind::Hook => "hook",
    }
}

/// The error for a `requires` entry no item satisfies, used at every place a `requires` list is
/// written and checked.
///
/// A project's entry and a catalog's own are the same grammar asking the same question, so an
/// entry that selects nothing is one mistake with one message, whether it names one item or globs
/// a whole prefix, and whether it was written in `ambit.yml`, in a pack, or in a `SKILL.md`.
///
/// A qualifier no catalog answers to is called out separately: the qualifier is an alias, not a
/// pattern, so a wildcard written there asks for a catalog literally named `*`, and a message about
/// "nothing matching the rest of the address" would be answering the wrong question.
///
/// Public because three surfaces reject an entry (the project check and the closure, each on the
/// first offender, and validation on every one of them) and the message must read identically from
/// all of them.
///
/// `within` is the catalog the entry resolves in: named by a project entry ([`entry_catalog`]), or
/// the catalog holding the requirer when the entry is a catalog's own. Always exactly one catalog,
/// never "whichever one happens to hold a match". `where` is the `(file line N)` suffix, as
/// [`at`] renders it; `catalogs` is every catalog the config listed, in config order.
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

    // An entry with no qualifier is a catalog's own, so this line only applies there: another
    // catalog holding a match does not count, since a catalog's own requires resolves within it.
    if entry.catalog.is_none() {
        detail.push(format!(
            "a catalog's own `{REQUIRES_KEY}` resolves within that catalog, which can only require what it ships"
        ));
    }

    detail.push("correct the pattern, add the item to a catalog, or remove the entry".to_owned());

    resolution_error(summary, detail)
}

/// The catalog a project's `requires` entry selects from.
///
/// Non-optional, unlike [`PatternEntry::catalog`]: a project config is parsed as qualified, so an
/// entry naming no alias is refused at parse and cannot reach here. The `expect` records that
/// invariant rather than inventing a fallback.
///
/// # Panics
///
/// When `entry` names no catalog, which only an unqualified entry from outside a project config
/// can.
///
/// Public so validation and resolution name the same catalog in the same words.
pub fn entry_catalog(entry: &PatternEntry) -> &str {
    entry
        .catalog
        .as_deref()
        .expect("a project's `requires` entry is parsed as qualified")
}

/// Rejects a `requires` entry that selects nothing.
///
/// An entry that matches nothing would yield a bundle quietly missing everything it was meant to
/// bring, so it fails loudly instead.
///
/// Stops at the first offender, sorted by `format_entry`, so which of several bad entries is
/// reported depends on what they say and not on config order. Listing every problem at once is
/// validation's job, which reuses the same error builder.
///
/// # Errors
///
/// Exit 3, naming the entry and the config line it was written on.
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

/// Where an entry was written, as [`at`] renders it: the file alone if the parse gave no line.
///
/// `ConfigOrigin::entry_lines` is keyed by [`entry_yaml`], the entry rendered whole.
///
/// Public so validation positions an entry exactly as resolution does.
pub fn entry_position(config: &ProjectConfig, entry: &PatternEntry) -> String {
    at(
        &config.origin.file,
        config.origin.entry_lines.get(&entry_yaml(entry)).copied(),
    )
}

/// Where a requirer's `requires` entry was written, as [`at`] renders it.
///
/// The file, and no line: a catalog's documents are parsed long before an entry is judged, so
/// nothing keeps the line it sat on (unlike a project's, which `ConfigOrigin` records). The file is
/// enough to act on, since the entry itself is quoted in the message.
///
/// Public so validation positions a catalog's entry exactly as the closure does.
pub fn requirer_position(requirer: &Requirer) -> String {
    at(&requirer.file, None)
}

/// A requirer's `requires` list, deduplicated and sorted by `entry_yaml` rather than kept in
/// authoring order, so which of several problems in a list gets reported does not depend on write
/// order.
///
/// Public so validation walks a list exactly as the closure does.
pub fn required_entries(requirer: &Requirer) -> Vec<PatternEntry> {
    let mut entries: Vec<(String, PatternEntry)> = unique_entries(&requirer.requires)
        .into_iter()
        .map(|entry| (entry_yaml(&entry), entry))
        .collect();

    entries.sort_by(|(a, _), (b, _)| js_cmp(a, b));
    entries.into_iter().map(|(_, entry)| entry).collect()
}

/// Every item one of a requirer's `requires` entries selects, matched against the catalog that
/// requirer came from and against nothing else.
///
/// A catalog is self-contained: an entry written inside one resolves within it, so `core.*` in
/// `company`'s `packs/engineering.yml` reaches only `company`'s items. A catalog author cannot
/// write a consumer's alias, so a bare pattern inside a catalog can only mean "my own catalog".
///
/// The locality is enforced here rather than in [`matches`], which skips its catalog test for an
/// unqualified entry: an unqualified entry does not know which catalog it was written in, so the
/// rule has to live with whoever offers it items.
///
/// Results stay in the merged catalog's order, being a filter of it.
///
/// Public so validation follows an edge exactly as the closure does: its cycle hunt walks every
/// requirer in the catalog, not only the selected ones, and must not disagree about where an edge
/// goes.
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

/// Whether a selection holds nothing at all, in any namespace.
fn is_empty(selection: &Selection) -> bool {
    selection.packs.is_empty()
        && selection.skills.is_empty()
        && selection.mcps.is_empty()
        && selection.hooks.is_empty()
}

/// Whether a requirer's `requires` entry selects anything its own catalog ships.
///
/// The catalog-side counterpart of [`matches_anything`]: an entry that selects nothing is a
/// mistake. Public because validation asks it of every entry in a catalog, while the closure asks
/// it only of the ones a selected requirer declares.
pub fn matches_own_catalog(
    entry: &PatternEntry,
    requirer: &Requirer,
    merged: &MergedCatalog,
) -> bool {
    !is_empty(&required_items(entry, requirer, merged))
}

/// The error for a `requires` cycle: the whole path, and the edge that closed it.
///
/// The path shows the loop without deciding which member is "the" problem; printing only one name
/// would make that choice for the reader. The closing edge is named separately, with its file,
/// because that is the actionable part: removing it breaks the cycle.
///
/// Members print as `<kind>:<name>`, not bare names, because a pack and a skill may share a name,
/// and `core → core` would misread as one item requiring itself.
///
/// Only the closing edge is annotated with its entry. A pattern can close a loop without naming
/// anything explicitly in it (a skill matching its own `core.*` is a one-step cycle), so showing
/// the entry is the only way to make that visible.
///
/// `cycle` is the items around the loop, opening and closing on the same one; `requirer` is whose
/// `requires` closed the loop, and whose file holds the entry; `entry` is that entry.
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

/// The identity of a requirer within the walk: its namespace, its catalog and its name.
fn requirer_key(requirer: &Requirer) -> String {
    node_key(requirer.kind, &requirer.catalog, &requirer.name)
}

/// [`requirer_key`] for a node known only by its parts, so an edge can find its target.
fn node_key(kind: RequirerKind, catalog: &str, name: &str) -> String {
    format!("{kind}{KIND_SEPARATOR}{}", qualified_name(catalog, name))
}

/// The requirers an edge reaches: the packs and skills it selected, as walk nodes.
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

/// The closure's walk state; see [`close_over_requires`].
struct Closure<'m> {
    merged: &'m MergedCatalog,
    // Keyed by address, not bare name: a set of names would treat two catalogs' copies as one item.
    chosen_packs: HashSet<String>,
    chosen_skills: HashSet<String>,
    chosen_mcps: HashSet<String>,
    chosen_hooks: HashSet<String>,
    // Keyed the way the walk addresses requirers, so an edge can find its target node directly.
    requirers: IndexMap<String, Requirer>,
    // `path` is the chain currently being followed (meeting something already on it is a cycle);
    // `closed` is what has been followed to completion (revisiting that is just a shared
    // requirement).
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

            // Leaf namespaces: an MCP or a hook carries no requires, so joining the selection is
            // all there is to do.
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
                // Checked here rather than on entry to `follow`, because only here do we know which
                // entry the edge came from, and the cycle error needs to name it.
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

/// Closes a selection over `requires` until fixpoint: every pack, skill, MCP entity and hook that a
/// selected pack or skill requires joins the selection, whether or not the project's own entries
/// would have selected it.
///
/// Packs are the main case this exists for. A catalog says once what "engineering" means (these
/// skills, that server, those hooks, plus the `core` pack) and a project takes the whole of it with
/// one entry. A skill's own `requires` is the narrower case: a skill that is useless without a
/// company-context skill and a server declares so, so a project reaching it gets a working bundle
/// instead of a broken one. Hooks follow the same route for the same reason.
///
/// The closure is set-valued: a requirer's entry is the project's entry minus the qualifier, so it
/// may glob and answers with a set of items, not a single name. There is nothing to look up by key,
/// so there is no "missing" case, only an entry that matched nothing, reported the same way
/// [`unmatched_entry_error`] reports it for a project's own entries.
///
/// Packs and skills are the interior of the graph; servers and hooks are leaves and carry no
/// `requires`. Each entry resolves within the requirer's own catalog (see [`required_items`]).
///
/// Accepted cost: a wildcard `requires` means a catalog author adding an item changes what an
/// unrelated pack pulls in silently. Add `skills/core/internal-notes`, and every requirer naming
/// `skill: core.*` grows a dependency at install with no message anywhere. This is the same class
/// of hazard as the collision [`assert_no_collisions`] refuses, but silent rather than loud. It is
/// recorded, not fixed, because forbidding wildcards inside a catalog would make catalogs less
/// expressive than the projects consuming them.
///
/// One consequence: a pattern matches the requirer itself if it can, so something that requires
/// itself is a one-step cycle: the skill `core.a` cannot require `skill: core.*`. This is not
/// special-cased, so [`cycle_error`] names the entry that closed the loop.
///
/// `roots` is what the project's entries selected among the two requiring kinds, in the merged
/// catalog's order; `mcps` and `hooks` are what they already selected of the leaves; `merged` is
/// what requirements resolve against, one catalog of it at a time.
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

    // Filtering the merged lists, rather than collecting during the walk, keeps the result in the
    // merged catalog's order regardless of discovery order.
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

/// The error for one name two selected catalogs both provide.
///
/// A harness's layout is flat and not ambit's to change (Claude reads `.claude/skills/<name>`), so
/// both copies want one path. Dropping one silently is refused instead: no catalog outranks
/// another, so there is nothing to prefer either copy with, and a bundle quietly missing a
/// requested copy would be undebuggable.
///
/// A pack materializes nowhere and so has no path to collide over, but it is refused all the same:
/// a pack is addressable (`ambit why pack:core`), and two selected packs named `core` would leave
/// that question with two answers.
///
/// `catalogs` is every catalog providing the name, in catalog order.
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

/// Rejects two selected copies of one name within one namespace.
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

/// Rejects a selection holding two catalogs' copies of one name.
///
/// This is where the collision the merge left unarbitrated gets settled. The conflict is about
/// materialization, not selection: a name two catalogs ship costs nothing until a project selects
/// both copies and a harness is asked to hold them at one path.
///
/// Stops at the first offender, in namespace order then name order. The selection arrives sorted by
/// name and then catalog, so which collision gets reported depends only on the names.
///
/// # Errors
///
/// Exit 3, naming the item and every catalog that provides it.
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

/// How a reason reads in `--explain`, in the lock, and in `ambit why`.
pub fn format_reason(reason: &SelectionReason) -> String {
    match reason {
        SelectionReason::Selected { entry } => format_entry(entry),
        SelectionReason::RequiredBy { requirer } => {
            format!("required-by:{}", format_item(requirer))
        }
    }
}

/// The error for a bundle that cannot account for one of its own items.
///
/// Exit 1, not 3: no catalog or config input can produce this, since every item in a bundle
/// arrives through one of the two routes by construction. Reaching it means the selection and its
/// explanation disagree, which is a bug.
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

/// The `required-by` reason for an item: the first selected requirer whose `requires` matches it.
///
/// Tested with [`matches`], not equality, since a `requires` entry is a pattern. Only a requirer
/// from the item's own catalog can be the answer, because that is as far as a catalog's `requires`
/// reaches.
///
/// Recovered from the closure's result rather than recorded during its walk, so which requirer is
/// named depends only on the names (packs first, then skills, each in name order) and not on
/// discovery order. Several requirers may name one thing; any of them is a true answer.
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

/// The reason each selected item of one namespace carries.
///
/// A `Selected` entry beats a `RequiredBy` edge: the entry ends a chain, while the edge continues
/// one, so preferring it keeps the explanation as short as possible while staying true.
///
/// `items` are `(catalog, name)` pairs; `entries` is the project's `requires` list; `selected` is
/// the selected requirers, which is what a `requires` edge can come from.
///
/// # Errors
///
/// Exit 1 for an item neither route accounts for.
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

/// The reasons of the namespace `kind` names, so a lookup never has to know which map that is.
fn reasons_of(bundle: &Bundle, kind: ItemKind) -> &IndexMap<String, SelectionReason> {
    match kind {
        ItemKind::Pack => &bundle.reasons.packs,
        ItemKind::Skill => &bundle.reasons.skills,
        ItemKind::Mcp => &bundle.reasons.mcps,
        ItemKind::Hook => &bundle.reasons.hooks,
    }
}

/// Whether an item is in the bundle.
pub fn is_selected(bundle: &Bundle, item: &BundleItem) -> bool {
    reasons_of(bundle, item.kind).contains_key(&item.name)
}

/// Why one item of a bundle is in it.
///
/// # Errors
///
/// Exit 1 if the item is not in the bundle; check with [`is_selected`] first, so a name a user
/// typed is rejected as the resolution error it is.
pub fn reason_of<'b>(bundle: &'b Bundle, item: &BundleItem) -> Result<&'b SelectionReason> {
    reasons_of(bundle, item.kind)
        .get(&item.name)
        .ok_or_else(|| unexplainable(item, "it is not in the bundle".to_owned()))
}

/// The whole chain behind one selected item, root cause first and the item itself last.
///
/// A reason alone is only half an answer: `required-by:pack:engineering` just raises the same
/// question one level up. The walk follows `required-by` edges backwards until it reaches a root
/// (an entry of the project's own), which terminates because `requires` cycles were rejected
/// during closure.
///
/// # Errors
///
/// Exit 1 if the item is not in the bundle, or the chain fails to terminate.
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

        // Guards against a broken invariant, not a bad catalog: a repeat here would mean a
        // `requires` cycle survived closure. Looping forever would be a worse way to report that.
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

/// Computes the bundle for a project.
///
/// Selection order comes from the merged catalog, which is already sorted by name, so filtering
/// preserves it.
///
/// `expects` is unioned over the closed selection, not just the entry-selected one: a server or
/// hook pulled in by a pack needs its credentials checked as much as one an entry named directly.
/// Packs contribute none, since a pack reads nothing from the world.
///
/// Reasons are computed here rather than on request, so `--explain`, `ambit why`, and the lock all
/// report the same answer, and a bundle that cannot account for an item fails at resolution instead
/// of at whichever surface asks first.
///
/// `merged` is every configured catalog. A project that ships items of its own lists itself as a
/// catalog, so all four namespaces arrive here the same way.
///
/// # Errors
///
/// Exit 3 for a `requires` entry that matches nothing, a `requires` cycle, or one name selected
/// from two catalogs.
pub fn resolve_bundle(config: &ProjectConfig, merged: &MergedCatalog) -> Result<Bundle> {
    // Checked first, before anything is selected, so an install cannot half-run on a config that
    // asked for something no catalog has.
    assert_entries_match(config, merged)?;
    let entries = &config.requires;
    let selects = |kind: ItemKind, catalog: &str, name: &str| {
        selecting_entry(entries, kind, catalog, name).is_some()
    };

    // Seed lists stay in the merged catalog's order, being a filter of it, so how something was
    // selected does not affect where it lands in the bundle.
    //
    // Selection is per copy, not per name: an entry reaching two catalogs' copies of one name
    // selects both, leaving the collision for the project to resolve.
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

    // Checked before the bundle exists and before any map below keys on a bare name: this is what
    // makes a name an identity from here on.
    assert_no_collisions(&selection)?;
    let Selection {
        packs,
        skills,
        mcps,
        hooks,
    } = selection;

    // The requirers that survived the closure, which is what a `required-by` reason may name.
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

#[cfg(all(test, feature = "cli"))]
mod tests;
