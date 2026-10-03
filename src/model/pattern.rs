//! The `requires` grammar: one-key mappings naming a namespace and a glob over names in it.

use crate::errors::Result;
use crate::model::requirement::ItemKind;
use crate::model::yaml::YamlMapping;
use crate::util::string_enum;

/// The key a selection list is written under, in a project config, a pack, or a skill's `ambit:`
/// block. The documents differ in what an address may say, not in what the list means.
pub const REQUIRES_KEY: &str = "requires";

string_enum! {
    /// Which spelling of an address a `requires` list is written in.
    ///
    /// - `Qualified`: `<catalog>/<pattern>`, mandatory in a project config. Only a project declares
    ///   catalog aliases in `catalogs:`, so only a project can name one; without the qualifier,
    ///   `core.*` would depend on catalog order.
    /// - `Unqualified`: the bare pattern, mandatory inside a catalog. A catalog author cannot write
    ///   the alias correctly, since it belongs to the consumer's config. A pack's or a skill's
    ///   `requires` therefore names its siblings unqualified and resolves within its own catalog,
    ///   which keeps a catalog self-contained (enforced in `resolution/resolve.rs`).
    ///
    /// A qualifier where it is refused, or a missing one where it is required, is exit 2 naming the
    /// key and line, rather than a value resolved against a guess.
    pub enum Addressing {
        Qualified => "qualified",
        Unqualified => "unqualified",
    }
}

/// One entry of a `requires` list: which namespace to select from, and the glob to select with.
///
/// An entry is a question about a catalog, answered by zero or more items; a bundle item is one
/// item. `- skill: core.*` names a namespace and a pattern; `skill:core.a` names a single item.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PatternEntry {
    /// The namespace this entry selects from: the entry's one key.
    pub kind: ItemKind,
    /// The glob, with the qualifier stripped off; see [`matches_pattern`].
    ///
    /// Never holds a [`CATALOG_SEPARATOR`](crate::model::requirement::CATALOG_SEPARATOR): the
    /// address is split at parse time, so everything downstream matches a pattern against a name
    /// and never has to re-split anything.
    pub pattern: String,
    /// The catalog alias the pattern is qualified with, present only when the entry was parsed as
    /// [`Addressing::Qualified`].
    ///
    /// Absent does not mean "any catalog": an unqualified entry is catalog-blind, and it is the
    /// caller resolving one (a catalog's own `requires`) that must restrict it to that catalog's
    /// items. [`matches`] cannot enforce this, since an unqualified entry carries no catalog.
    pub catalog: Option<String>,
}

/// One item, of one namespace, as a pattern is matched against it.
///
/// A structural shape rather than a merged-catalog type, so this module does not need to know how
/// an item is loaded: every merged item can lend one once the caller says which namespace it is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternItem<'a> {
    /// Which namespace this item is in.
    pub kind: ItemKind,
    /// The catalog the item came from, which a qualified entry is matched against.
    pub catalog: &'a str,
    /// The item's name inside its namespace, dotted: `core.house-style`.
    pub name: &'a str,
}

/// Whether `pattern` matches `text`.
///
/// `*` matches any run of characters, **including `.`**, anywhere, any number of times. A pattern
/// holding no `*` is an exact name.
///
/// ```text
/// core.*  ->  core.a, core.a.b     (NOT core)
/// core    ->  core
/// *       ->  everything
/// ```
///
/// **`core.*` excludes `core` itself.** The pattern says *`core`, a dot, then anything*, and `core`
/// has no dot. Selecting a prefix and the item named exactly that therefore takes two entries.
///
/// `*` spans `.` because a catalog's namespaces are flat: the dot in a name is a naming
/// convention, not a structural separator this grammar understands.
pub fn matches_pattern(pattern: &str, text: &str) -> bool {
    let _ = (pattern, text);
    todo!("port model/pattern.ts:matchesPattern")
}

/// The address as it was written: `company/core.*` when qualified, `core.*` when not.
///
/// Recomposed from the parsed halves rather than kept as a third field, so there is one
/// representation of the entry and no way for the two to disagree.
pub fn entry_address(entry: &PatternEntry) -> String {
    let _ = entry;
    todo!("port model/pattern.ts:entryAddress")
}

/// How an entry is written where only a string will do: `pack:company/engineering`,
/// `skill:company/core.*`.
///
/// The same `<kind>:<name>` shape `ambit why` takes as its subject.
pub fn format_entry(entry: &PatternEntry) -> String {
    let _ = entry;
    todo!("port model/pattern.ts:formatEntry")
}

/// How an entry is written in a document, for a message telling someone to write one.
///
/// A one-key mapping fits block style on one line, so this is the entry exactly as it belongs in a
/// `requires` list. The pattern is quoted unconditionally, since it is exactly the kind of string
/// YAML would otherwise read as something else.
pub fn entry_yaml(entry: &PatternEntry) -> String {
    let _ = entry;
    todo!("port model/pattern.ts:entryYaml")
}

/// Whether two entries say literally the same thing: the same namespace and the same address.
///
/// Exact only. `skill: core.*` does **not** absorb `skill: core.a`. Subsumption is not
/// implemented, so two entries where one is redundant simply stay two entries.
pub fn same_entry(a: &PatternEntry, b: &PatternEntry) -> bool {
    let _ = (a, b);
    todo!("port model/pattern.ts:sameEntry")
}

/// A `requires` list with literal duplicates dropped, keeping the first of each and the order the
/// list was written in.
///
/// Order-preserving rather than sorted, because this reads a list rather than rewriting one.
/// Anything downstream needing a total order can sort over [`format_entry`].
pub fn unique_entries(entries: &[PatternEntry]) -> Vec<PatternEntry> {
    let _ = entries;
    todo!("port model/pattern.ts:uniqueEntries")
}

/// Whether `entry` selects `item`.
///
/// Three tests, all of which have to hold: the item's namespace is the one the entry named, the
/// item's catalog is the one the entry qualified (when it qualified one; see
/// [`PatternEntry::catalog`]), and the pattern matches the item's name.
pub fn matches(entry: &PatternEntry, item: PatternItem<'_>) -> bool {
    let _ = (entry, item);
    todo!("port model/pattern.ts:matches")
}

/// Parses a `requires` list: a sequence of one-key mappings, each naming a namespace and the
/// pattern to match names in it.
///
/// Returned in the order it was written, duplicates included. Deduplication is
/// [`unique_entries`], kept separate because a caller merging several lists wants to dedupe the
/// union rather than each part.
///
/// `mapping` is the block the key sits in: a project's config root, a pack's document, a skill's
/// `ambit:`.
///
/// # Errors
///
/// Exit 2 for an entry that is not a mapping, one naming no namespace or more than one, one
/// carrying a key this grammar does not have, or an address the spelling refuses.
pub fn parse_entries(mapping: &YamlMapping, addressing: Addressing) -> Result<Vec<PatternEntry>> {
    let _ = (mapping, addressing);
    todo!("port model/pattern.ts:parseEntries")
}
