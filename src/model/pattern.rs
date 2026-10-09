use std::sync::LazyLock;

use crate::errors::{AmbitError, Result, at, config_error};
use crate::model::requirement::{CATALOG_SEPARATOR, ITEM_KINDS, ItemKind};
use crate::model::yaml::{PositionedString, YamlEntry, YamlMapping};
use crate::util::string_enum;

pub const REQUIRES_KEY: &str = "requires";

string_enum! {
    pub enum Addressing {
        Qualified => "qualified",
        Unqualified => "unqualified",
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PatternEntry {
    pub kind: ItemKind,
    pub pattern: String,
    pub catalog: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternItem<'a> {
    pub kind: ItemKind,
    pub catalog: &'a str,
    pub name: &'a str,
}

const WILDCARD: &str = "*";

pub fn matches_pattern(pattern: &str, text: &str) -> bool {
    let literals: Vec<&str> = pattern.split(WILDCARD).collect();

    let [first, middle @ .., last] = literals.as_slice() else {
        return pattern == text;
    };

    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };

    for literal in middle {
        let Some(index) = rest.find(literal) else {
            return false;
        };

        rest = &rest[index + literal.len()..];
    }

    rest.ends_with(last)
}

const KIND_SEPARATOR: &str = ":";

pub fn entry_address(entry: &PatternEntry) -> String {
    match &entry.catalog {
        None => entry.pattern.clone(),
        Some(catalog) => format!("{catalog}{CATALOG_SEPARATOR}{}", entry.pattern),
    }
}

pub fn format_entry(entry: &PatternEntry) -> String {
    format!("{}{KIND_SEPARATOR}{}", entry.kind, entry_address(entry))
}

pub fn entry_yaml(entry: &PatternEntry) -> String {
    format!("- {}: \"{}\"", entry.kind, entry_address(entry))
}

pub fn same_entry(a: &PatternEntry, b: &PatternEntry) -> bool {
    a.kind == b.kind && a.pattern == b.pattern && a.catalog == b.catalog
}

pub fn unique_entries(entries: &[PatternEntry]) -> Vec<PatternEntry> {
    let mut kept: Vec<PatternEntry> = Vec::new();

    for entry in entries {
        if !kept.iter().any(|seen| same_entry(seen, entry)) {
            kept.push(entry.clone());
        }
    }

    kept
}

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

static KIND_LIST: LazyLock<String> = LazyLock::new(|| {
    ITEM_KINDS
        .iter()
        .map(|kind| format!("`{kind}`"))
        .collect::<Vec<_>>()
        .join(", ")
});

const ALIAS_PLACEHOLDER: &str = "<catalog>";

fn shown_entry(kind: ItemKind, pattern: &str) -> String {
    entry_yaml(&PatternEntry {
        kind,
        pattern: pattern.to_owned(),
        catalog: None,
    })
}

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

fn bad_address(
    entry: &YamlMapping,
    kind: ItemKind,
    address: &str,
    addressing: Addressing,
    problem: &str,
    fix: String,
) -> AmbitError {
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

    entry.key_error(
        kind.as_str(),
        &format!("`{REQUIRES_KEY}` entry \"{address}\" {problem}"),
        detail,
    )
}

fn split_address(
    entry: &YamlMapping,
    kind: ItemKind,
    address: &str,
    addressing: Addressing,
) -> Result<(Option<String>, String)> {
    let parts: Vec<&str> = address.split(CATALOG_SEPARATOR).collect();

    if addressing == Addressing::Unqualified {
        if parts.len() == 1 {
            return Ok((None, address.to_owned()));
        }

        return Err(bad_address(
            entry,
            kind,
            address,
            addressing,
            "names a catalog, which a catalog's own `requires` may not",
            example(kind, address, addressing),
        ));
    }

    if parts.len() == 1 {
        return Err(bad_address(
            entry,
            kind,
            address,
            addressing,
            "names no catalog",
            format!(
                "qualify it: `<catalog>{CATALOG_SEPARATOR}{address}`, using an alias from `catalogs:`"
            ),
        ));
    }

    if parts.len() > 2 {
        return Err(bad_address(
            entry,
            kind,
            address,
            addressing,
            &format!("holds {} `{CATALOG_SEPARATOR}` separators", parts.len() - 1),
            "an item's name holds none, so remove all but the first".to_owned(),
        ));
    }

    let (catalog, pattern) = (parts[0], parts[1]);

    if catalog.is_empty() {
        return Err(bad_address(
            entry,
            kind,
            address,
            addressing,
            "names an empty catalog",
            format!("write the alias before the `{CATALOG_SEPARATOR}`"),
        ));
    }

    if pattern.is_empty() {
        return Err(bad_address(
            entry,
            kind,
            address,
            addressing,
            "names an empty pattern",
            format!("write `{catalog}{CATALOG_SEPARATOR}{WILDCARD}` for the whole catalog"),
        ));
    }

    Ok((Some(catalog.to_owned()), pattern.to_owned()))
}

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

pub fn parse_entries(mapping: &YamlMapping, addressing: Addressing) -> Result<Vec<PatternEntry>> {
    let Some(items) = mapping.optional_entry_list(REQUIRES_KEY)? else {
        return Ok(Vec::new());
    };

    items
        .iter()
        .map(|item| match item {
            YamlEntry::String(item) => Err(bare_entry(mapping, item, addressing)),
            YamlEntry::Mapping(entry) => parse_entry(entry, addressing),
        })
        .collect()
}

#[cfg(test)]
mod tests;
