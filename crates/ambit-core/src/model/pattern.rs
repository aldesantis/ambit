//! How a selection is written: the `requires` entry and its glob.
//!
//! Used the same way in a project's `requires`, a pack's `requires`, and a skill's `requires`; only
//! whether the address carries a catalog differs (see [`Addressing`]).
//!
//! ```yaml
//! requires:
//!   - pack: "company/engineering" # everything that pack pulls in, transitively
//!   - skill: "company/core.*" # everything beneath the `core` name prefix
//!   - hook: "company/guards.*"
//! ```
//!
//! An entry is a one-key mapping: the key is the [`ItemKind`] being selected, and the value is the
//! glob to match names in that namespace. There is nothing else to match on, since a catalog item
//! has only a name.
//!
//! The namespace key is mandatory because a catalog's namespaces are flat and independent: a skill
//! at `skills/mcp/sentry/SKILL.md` can be named `mcp.sentry` while an unrelated MCP entity is also
//! called `sentry`. `- mcp.sentry` alone cannot say which is meant.
//!
//! No bare shorthand: `- "company/core.*"` is refused rather than resolved against a guessed
//! namespace.
//!
//! No negation: `!company/core.internal.*` is not part of this grammar, so a leading `!` is matched
//! literally. A pattern matching nothing is an error at resolve time.
//!
//! This module is pure: nothing here reads a catalog. Matching is a function of the entry and one
//! item. What a pattern matching nothing means, which items a catalog's own `requires` may see, and
//! how a reason is rendered live in the resolution code that asks those questions.

use std::sync::LazyLock;

use crate::errors::{AmbitError, Result, at, config_error};
use crate::model::requirement::{CATALOG_SEPARATOR, ITEM_KINDS, ItemKind};
use crate::model::yaml::{PositionedString, YamlEntry, YamlMapping};
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
    ///   the alias correctly, since it belongs to the consumer's config and the same catalog is
    ///   `company` in one project and `acme` in the next. A pack's or a skill's `requires` therefore
    ///   names its siblings unqualified and resolves within its own catalog, which keeps a catalog
    ///   self-contained: it can only require what it ships (enforced in `resolution/resolve.rs`).
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
    /// Never holds a [`CATALOG_SEPARATOR`]: the address is split at parse time, so everything
    /// downstream matches a pattern against a name and never has to re-split anything.
    pub pattern: String,
    /// The catalog alias the pattern is qualified with, present only when the entry was parsed as
    /// [`Addressing::Qualified`].
    ///
    /// Absent does not mean "any catalog": an unqualified entry is catalog-blind, and it is the
    /// caller resolving one (a catalog's own `requires`) that must restrict it to that catalog's
    /// items. [`matches`] cannot enforce this, since an unqualified entry carries no catalog to
    /// check.
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

/// The one metacharacter this grammar has.
const WILDCARD: &str = "*";

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
/// has no dot. Selecting a prefix and the item named exactly that therefore takes two entries; the
/// omission of the bare name is silent.
///
/// `*` spans `.` because a catalog's namespaces are flat: the dot in a name is a naming convention,
/// not a structural separator this grammar understands. So `core.*` reaches `core.a.b` in one
/// entry.
///
/// Every character other than `*` is literal, and that includes newlines: a name holding one is
/// pathological, but the matcher still honors "any run of characters" consistently.
pub fn matches_pattern(pattern: &str, text: &str) -> bool {
    let literals: Vec<&str> = pattern.split(WILDCARD).collect();

    // No wildcard: an exact name.
    let [first, middle @ .., last] = literals.as_slice() else {
        return pattern == text;
    };

    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };

    // Leftmost matches are always safe for a grammar whose only metacharacter is `*`: consuming
    // less of the text can never make a later literal harder to find.
    for literal in middle {
        let Some(index) = rest.find(literal) else {
            return false;
        };

        rest = &rest[index + literal.len()..];
    }

    rest.ends_with(last)
}

/// Whether a pattern is an exact name: one holding no wildcard.
///
/// An entry with a literal pattern selects at most one item per catalog; anything else is a rule
/// whose matches can change whenever the catalog does.
#[allow(dead_code)] // Reached by the desktop app's FFI layer, not by the CLI binary.
pub fn is_literal(pattern: &str) -> bool {
    !pattern.contains(WILDCARD)
}

/// What separates a kind from the address it applies to, where only a string will do.
const KIND_SEPARATOR: &str = ":";

/// The address as it was written: `company/core.*` when qualified, `core.*` when not.
///
/// Recomposed from the parsed halves rather than kept as a third field, so there is one
/// representation of the entry and no way for the two to disagree.
pub fn entry_address(entry: &PatternEntry) -> String {
    match &entry.catalog {
        None => entry.pattern.clone(),
        Some(catalog) => format!("{catalog}{CATALOG_SEPARATOR}{}", entry.pattern),
    }
}

/// How an entry is written where only a string will do: `pack:company/engineering`,
/// `skill:company/core.*`.
///
/// The same `<kind>:<name>` shape `ambit why` takes as its subject: an entry and an item are named
/// by the same grammar, one carrying a pattern where the other carries a name.
pub fn format_entry(entry: &PatternEntry) -> String {
    format!("{}{KIND_SEPARATOR}{}", entry.kind, entry_address(entry))
}

/// How an entry is written in a document, for a message telling someone to write one.
///
/// A one-key mapping fits block style on one line, so this is the entry exactly as it belongs in a
/// `requires` list. The pattern is quoted unconditionally, since it is exactly the kind of string
/// YAML would otherwise read as something else.
pub fn entry_yaml(entry: &PatternEntry) -> String {
    format!("- {}: \"{}\"", entry.kind, entry_address(entry))
}

/// Whether two entries say literally the same thing: the same namespace and the same address.
///
/// Exact only. `skill: core.*` does **not** absorb `skill: core.a`, even though everything the
/// second selects the first selects too. Subsumption is not implemented, so two entries where one
/// is redundant simply stay two entries.
pub fn same_entry(a: &PatternEntry, b: &PatternEntry) -> bool {
    a.kind == b.kind && a.pattern == b.pattern && a.catalog == b.catalog
}

/// A `requires` list with literal duplicates dropped, keeping the first of each and the order the
/// list was written in.
///
/// Order-preserving rather than sorted, because this reads a list rather than rewriting one:
/// sorting a document's own entries would be reformatting the author did not ask for. Anything
/// downstream needing a total order can sort over [`format_entry`].
pub fn unique_entries(entries: &[PatternEntry]) -> Vec<PatternEntry> {
    let mut kept: Vec<PatternEntry> = Vec::new();

    for entry in entries {
        if !kept.iter().any(|seen| same_entry(seen, entry)) {
            kept.push(entry.clone());
        }
    }

    kept
}

/// Whether `entry` selects `item`.
///
/// Three tests, all of which have to hold: the item's namespace is the one the entry named, the
/// item's catalog is the one the entry qualified (when it qualified one; see
/// [`PatternEntry::catalog`]), and the pattern matches the item's name.
pub fn matches(entry: &PatternEntry, item: PatternItem<'_>) -> bool {
    if entry.kind != item.kind {
        return false;
    }

    if let Some(catalog) = &entry.catalog
        && catalog != item.catalog
    {
        return false;
    }

    matches_pattern(&entry.pattern, item.name)
}

/// The namespaces as a message lists them: `` `pack`, `skill`, `mcp`, `hook` ``.
static KIND_LIST: LazyLock<String> = LazyLock::new(|| {
    ITEM_KINDS
        .iter()
        .map(|kind| format!("`{kind}`"))
        .collect::<Vec<_>>()
        .join(", ")
});

/// Stands in for a catalog alias a refusal has no way to know.
///
/// Literal rather than guessed: the alias is the reader's to pick, from their own `catalogs:`.
const ALIAS_PLACEHOLDER: &str = "<catalog>";

/// An unqualified entry, for the messages that show one.
fn shown_entry(kind: ItemKind, pattern: &str) -> String {
    entry_yaml(&PatternEntry {
        kind,
        pattern: pattern.to_owned(),
        catalog: None,
    })
}

/// The example every refusal that has an address to work with ends on.
///
/// Written in the spelling the document being read demands, so a catalog author is never shown a
/// qualifier they cannot write, and a project is never shown a bare pattern it would refuse in
/// turn. [`ALIAS_PLACEHOLDER`] stands in where the reader has written no alias.
fn example(kind: ItemKind, address: &str, addressing: Addressing) -> String {
    let bare = address
        .rsplit(CATALOG_SEPARATOR)
        .next()
        .expect("split yields at least one part");
    let shown = match addressing {
        Addressing::Unqualified => bare.to_owned(),
        Addressing::Qualified if address.contains(CATALOG_SEPARATOR) => address.to_owned(),
        Addressing::Qualified => format!("{ALIAS_PLACEHOLDER}{CATALOG_SEPARATOR}{address}"),
    };

    format!("write it as `{}`", shown_entry(kind, &shown))
}

/// The error for an entry written as a bare string: the shape a plain list of patterns has.
///
/// Names what a bare pattern fails to say rather than guessing it, since the namespace is the whole
/// of what this grammar declares and there is no shorthand for it.
fn bare_entry(
    mapping: &YamlMapping,
    item: &PositionedString,
    addressing: Addressing,
) -> AmbitError {
    let line = item.line.or_else(|| mapping.line_of(REQUIRES_KEY));

    config_error(
        format!(
            "`{REQUIRES_KEY}` entry \"{}\" is not a mapping {}",
            item.value,
            at(mapping.file(), line)
        ),
        [
            format!(
                "a bare pattern does not say which namespace it selects from ({})",
                *KIND_LIST
            ),
            example(ItemKind::Skill, &item.value, addressing),
        ],
    )
}

/// The error for an entry naming no namespace, or more than one.
fn bad_kind(entry: &YamlMapping, declared: &[ItemKind]) -> AmbitError {
    let advice = vec![
        format!(
            "an entry is one key naming a namespace, carrying the pattern to match names in it: {}",
            *KIND_LIST
        ),
        if declared.is_empty() {
            format!(
                "write it as `{}`",
                shown_entry(ItemKind::Skill, "<pattern>")
            )
        } else {
            "split it into one entry per namespace".to_owned()
        },
    ];

    match declared.first() {
        None => config_error(
            format!(
                "`{REQUIRES_KEY}` entry selects from no namespace {}",
                at(entry.file(), entry.line())
            ),
            advice,
        ),
        Some(first) => entry.key_error(
            first.as_str(),
            &format!(
                "`{REQUIRES_KEY}` entry selects from {} namespaces: {}",
                declared.len(),
                declared
                    .iter()
                    .map(|kind| kind.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            advice,
        ),
    }
}

/// Why an address is refused under the demanded [`Addressing`]: what is wrong with it, and how to
/// write it instead.
struct AddressProblem {
    problem: String,
    fix: String,
}

/// The detail lines of a refused address: why the spelling is demanded, then the fix.
fn address_detail(addressing: Addressing, fix: String) -> Vec<String> {
    let mut detail = match addressing {
        Addressing::Qualified => vec![format!(
            "a project selects from a catalog it listed in `catalogs:`, so an address is `<catalog>{CATALOG_SEPARATOR}<pattern>`"
        )],
        Addressing::Unqualified => vec![
            "a catalog author cannot write the alias: it belongs to the consumer's config, and the same catalog is `company` in one project and `acme` in the next".to_owned(),
            "a catalog's own `requires` resolves within that catalog, so the pattern stands alone".to_owned(),
        ],
    };

    detail.push(fix);
    detail
}

/// The summary line of a refused address, without a position.
fn address_summary(address: &str, problem: &str) -> String {
    format!("`{REQUIRES_KEY}` entry \"{address}\" {problem}")
}

/// The catalog and the pattern an address holds, under the spelling the document demands.
///
/// Split here, once, so [`PatternEntry::pattern`] never carries a [`CATALOG_SEPARATOR`] and nothing
/// downstream re-derives the halves. A `/` is refused inside the pattern half in both spellings: an
/// item's name is a dotted path and holds none, so a second separator is a stray qualifier however
/// it got there.
///
/// Positionless, so an address that never sat in a document ([`parse_address`]) is judged by the
/// same rules as one that did ([`split_address`]).
fn address_parts(
    kind: ItemKind,
    address: &str,
    addressing: Addressing,
) -> std::result::Result<(Option<String>, String), AddressProblem> {
    let parts: Vec<&str> = address.split(CATALOG_SEPARATOR).collect();
    let refuse = |problem: &str, fix: String| AddressProblem {
        problem: problem.to_owned(),
        fix,
    };

    if addressing == Addressing::Unqualified {
        if parts.len() == 1 {
            return Ok((None, address.to_owned()));
        }

        return Err(refuse(
            "names a catalog, which a catalog's own `requires` may not",
            example(kind, address, addressing),
        ));
    }

    if parts.len() == 1 {
        return Err(refuse(
            "names no catalog",
            format!(
                "qualify it: `<catalog>{CATALOG_SEPARATOR}{address}`, using an alias from `catalogs:`"
            ),
        ));
    }

    if parts.len() > 2 {
        return Err(refuse(
            &format!("holds {} `{CATALOG_SEPARATOR}` separators", parts.len() - 1),
            "an item's name holds none, so remove all but the first".to_owned(),
        ));
    }

    let (catalog, pattern) = (parts[0], parts[1]);

    if catalog.is_empty() {
        return Err(refuse(
            "names an empty catalog",
            format!("write the alias before the `{CATALOG_SEPARATOR}`"),
        ));
    }

    if pattern.is_empty() {
        return Err(refuse(
            "names an empty pattern",
            format!("write `{catalog}{CATALOG_SEPARATOR}{WILDCARD}` for the whole catalog"),
        ));
    }

    Ok((Some(catalog.to_owned()), pattern.to_owned()))
}

/// [`address_parts`] for an address read from a document, refused with the key and line.
fn split_address(
    entry: &YamlMapping,
    kind: ItemKind,
    address: &str,
    addressing: Addressing,
) -> Result<(Option<String>, String)> {
    address_parts(kind, address, addressing).map_err(|refused| {
        entry.key_error(
            kind.as_str(),
            &address_summary(address, &refused.problem),
            address_detail(addressing, refused.fix),
        )
    })
}

/// Parses one entry that was never written in a document, such as a rule typed into a form.
///
/// The same grammar and the same refusals as [`parse_entries`], minus the position: there is no
/// file or line to name.
///
/// # Errors
///
/// Exit 2 for an address the spelling refuses.
#[allow(dead_code)] // Reached by the desktop app's FFI layer, not by the CLI binary.
pub fn parse_address(
    kind: ItemKind,
    address: &str,
    addressing: Addressing,
) -> Result<PatternEntry> {
    let (catalog, pattern) = address_parts(kind, address, addressing).map_err(|refused| {
        config_error(
            address_summary(address, &refused.problem),
            address_detail(addressing, refused.fix),
        )
    })?;

    Ok(PatternEntry {
        kind,
        pattern,
        catalog,
    })
}

/// Parses one `requires` entry: a one-key mapping naming a namespace and carrying a pattern.
///
/// Unknown keys are rejected first, so a leftover `tag:` or `capabilities:` reads as an unknown key
/// rather than as an entry naming no namespace.
fn parse_entry(entry: &YamlMapping, addressing: Addressing) -> Result<PatternEntry> {
    let names: Vec<&str> = ITEM_KINDS.iter().map(|kind| kind.as_str()).collect();

    entry.reject_unknown_keys(&names)?;

    let declared: Vec<ItemKind> = ITEM_KINDS
        .iter()
        .copied()
        .filter(|candidate| entry.has(candidate.as_str()))
        .collect();

    let [kind] = declared.as_slice() else {
        return Err(bad_kind(entry, &declared));
    };

    let address = entry.require_string(kind.as_str())?;
    let (catalog, pattern) = split_address(entry, *kind, &address, addressing)?;

    Ok(PatternEntry {
        kind: *kind,
        pattern,
        catalog,
    })
}

/// Parses a `requires` list: a sequence of one-key mappings, each naming a namespace and the
/// pattern to match names in it.
///
/// Returned in the order it was written, duplicates included. Deduplication is
/// [`unique_entries`], kept as a separate step because a caller merging several lists wants to
/// dedupe the union rather than each part.
///
/// `mapping` is the block the key sits in: a project's config root, a pack's document, a skill's
/// `ambit:`. `addressing` is which spelling this document demands; see [`Addressing`].
///
/// # Errors
///
/// Exit 2 for an entry that is not a mapping, one naming no namespace or more than one, one
/// carrying a key this grammar does not have, or an address the spelling refuses.
pub fn parse_entries(mapping: &YamlMapping, addressing: Addressing) -> Result<Vec<PatternEntry>> {
    let Some(items) = mapping.optional_entry_list(REQUIRES_KEY)? else {
        return Ok(Vec::new());
    };

    items
        .iter()
        .map(|item| match item {
            // A string is a bare pattern; everything else the sequence could hold was already
            // refused by `optional_entry_list`.
            YamlEntry::String(item) => Err(bare_entry(mapping, item, addressing)),
            YamlEntry::Mapping(entry) => parse_entry(entry, addressing),
        })
        .collect()
}

#[cfg(test)]
mod tests;
