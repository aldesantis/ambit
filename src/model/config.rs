//! `ambit.yml`: the project config.
//!
//! Parsing is total: whatever comes back is fully typed and needs no further checking, and
//! anything the config could not express has already been rejected with an exit-2 error naming
//! the file, the key, and the line.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::{Result, at, config_error};
use crate::model::pattern::{
    Addressing, PatternEntry, REQUIRES_KEY, entry_yaml, parse_entries, unique_entries,
};
use crate::model::requirement::{CATALOG_SEPARATOR, ItemKind};
use crate::model::yaml::{YamlEntry, YamlMapping, read_yaml_mapping};
use crate::util::path::join;

/// The only config version this build understands.
pub const CONFIG_VERSION: i64 = 1;

/// Used when `harnesses` is absent.
pub const DEFAULT_HARNESSES: &[&str] = &["claude"];

/// Accepted config filenames, in preference order. Having both is an error.
///
/// The first is the one `ambit init` writes.
pub const CONFIG_FILENAMES: [&str; 2] = ["ambit.yml", "ambit.yaml"];

/// A catalog to fetch and parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRef {
    pub name: String,
    pub source: String,
    /// Tag, branch, or commit. Absent means the source's default branch.
    pub r#ref: Option<String>,
    /// The directory inside the source that holds the catalog, `/`-separated and normalized.
    /// Absent means the source's root.
    pub path: Option<String>,
}

/// Where the config came from, and where inside it the values live that a later stage judges.
///
/// Resolution runs long after parsing, so an error about a `requires` entry has no YAML node left
/// to point at, yet it still has to name the file and the line. This carries just enough of the
/// document's positions for that, keeping [`ProjectConfig`] itself plain data with no parser state
/// hanging off it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigOrigin {
    /// How the config file is named in messages: `ambit.yml` or `ambit.yaml`, project-relative.
    pub file: String,
    /// 1-based line each `requires` entry was written on, keyed by
    /// [`entry_yaml`](crate::model::pattern::entry_yaml).
    ///
    /// An entry renders to exactly one line, so the rendering is the key: two entries that render
    /// alike are the same selection, and the first line either was written on is the one a reader
    /// scanning downward finds.
    pub entry_lines: IndexMap<String, usize>,
}

/// A parsed, validated `ambit.yml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectConfig {
    pub version: i64,
    /// Positions for the errors raised after parsing.
    pub origin: ConfigOrigin,
    pub harnesses: Vec<String>,
    /// Catalogs to fetch and parse, in the order they were listed.
    ///
    /// The order carries no meaning: every catalog's copy of a name survives the merge, so there is
    /// no precedence between them to establish. It is kept because it is what the config says, and
    /// because the lock lists catalogs as inputs.
    pub catalogs: Vec<CatalogRef>,
    /// What this project selects: pattern entries in the order they were written, literal
    /// duplicates dropped.
    ///
    /// Deduplicated here because an entry written twice is one selection and one finding. The
    /// order is the document's, so nothing downstream has to sort to be deterministic.
    pub requires: Vec<PatternEntry>,
}

/// A config file found in a project directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundConfig {
    /// Absolute path.
    pub path: PathBuf,
    /// The project-relative name to use in messages.
    pub file: String,
}

const CONFIG_KEYS: &[&str] = &["catalogs", "harnesses", REQUIRES_KEY, "version"];
const CATALOG_KEYS: &[&str] = &["name", "path", "ref", "source"];

/// The second half of every rewrite below.
///
/// A definition lives in a file, and a file is only reachable through a catalog, so moving a
/// definition out of `ambit.yml` always takes two steps, and this is always the second one. The
/// catalog that holds a project's own files is the project.
const SELF_CATALOG_ADVICE: &str =
    "then list this project as a catalog: `- name: local` with `source: path:.`";

/// A key that used to carry a definition in `ambit.yml`, with the file each entry moves into.
struct RemovedInlineKey {
    key: &'static str,
    /// How the message names one of the entries.
    subject: &'static str,
    /// Where one of them lives now, relative to the catalog root.
    file: &'static str,
}

/// The keys that used to carry a definition in `ambit.yml`.
///
/// Kept only so their presence can be refused with a specific message.
/// [`YamlMapping::reject_unknown_keys`] would already stop a config that still writes one, but its
/// message says "unknown key" and lists the accepted set, which reads as a typo rather than telling
/// the reader where the definition went.
const REMOVED_INLINE_KEYS: &[RemovedInlineKey] = &[
    RemovedInlineKey {
        key: "mcps",
        subject: "an MCP server",
        file: "mcps/<name>.yml",
    },
    RemovedInlineKey {
        key: "hooks",
        subject: "a hook",
        file: "hooks/<name>/hook.yml",
    },
];

/// Refuses a top-level `mcps:` or `hooks:`, naming the file the definitions move into.
///
/// Runs before [`YamlMapping::reject_unknown_keys`] so this message fires first instead of the
/// generic "unknown key" one.
fn assert_no_inline_definitions(root: &YamlMapping) -> Result<()> {
    for removed in REMOVED_INLINE_KEYS {
        if !root.has(removed.key) {
            continue;
        }

        return Err(root.key_error(
            removed.key,
            &format!("top-level `{}` is gone", removed.key),
            vec![
                format!(
                    "{} is defined by a file of its own: move each entry to `{}`",
                    removed.subject, removed.file
                ),
                SELF_CATALOG_ADVICE.to_owned(),
            ],
        ));
    }

    Ok(())
}

/// A key a project used to select with, and the entry each of its members becomes.
struct RemovedSelectionKey {
    key: &'static str,
    /// Which namespace each member selected from, where one entry can still say it.
    kind: Option<ItemKind>,
}

/// The two keys a project used to select with.
///
/// Both are gone in favour of one `requires:` list of one-key entries. A `skills` entry selected
/// one skill by name, so it becomes `- skill:` qualified with an alias, and the rewrite is
/// mechanical enough to print. A held scope selected across every namespace at once by label,
/// which no single entry does any more (that job belongs to a pack declared in the catalog), so its
/// refusal explains that instead of printing a rewrite that would only be half true.
const REMOVED_SELECTION_KEYS: &[RemovedSelectionKey] = &[
    RemovedSelectionKey {
        key: "scopes",
        kind: None,
    },
    RemovedSelectionKey {
        key: "skills",
        kind: Some(ItemKind::Skill),
    },
];

/// Stands in for a catalog alias the config does not name unambiguously.
const ALIAS_PLACEHOLDER: &str = "<catalog>";

/// The catalog aliases this config declares, for the rewrite a removed key's refusal prints.
///
/// Read defensively rather than through [`parse_catalogs`]: a malformed `catalogs:` is refused on
/// its own terms once the removed key is gone, so a failure here should only cost the message a
/// concrete alias, nothing else.
fn catalog_aliases(root: &YamlMapping) -> Vec<String> {
    let Ok(entries) = root.optional_mapping_list("catalogs") else {
        return Vec::new();
    };

    let mut aliases = Vec::new();

    for entry in entries.unwrap_or_default() {
        match entry.optional_string("name") {
            Ok(Some(name)) => aliases.push(name),
            Ok(None) => {}
            Err(_) => return Vec::new(),
        }
    }

    aliases
}

/// Which alias a rewrite qualifies its patterns with.
///
/// The one the config declares, when it declares exactly one. With several there is nothing to
/// pick with: a held scope reached every catalog at once, and which of them a given entry should
/// now name is the reader's call, so the placeholder is used instead of guessing.
fn rewrite_alias(root: &YamlMapping) -> String {
    match catalog_aliases(root).as_slice() {
        [only] => only.clone(),
        _ => ALIAS_PLACEHOLDER.to_owned(),
    }
}

/// Refuses a top-level `scopes:` or `skills:`, naming the `requires` entry each member becomes.
///
/// Runs before [`YamlMapping::reject_unknown_keys`] for the same reason
/// [`assert_no_inline_definitions`] does: the generic "unknown key" message would say nothing
/// about where the selection went.
fn assert_no_removed_selection(root: &YamlMapping) -> Result<()> {
    for removed in REMOVED_SELECTION_KEYS {
        if !root.has(removed.key) {
            continue;
        }

        let catalog = rewrite_alias(root);
        let rewrites: Vec<String> = match removed.kind {
            None => Vec::new(),
            Some(kind) => root
                .optional_positioned_string_list(removed.key)?
                .unwrap_or_default()
                .into_iter()
                .map(|entry| {
                    let yaml = entry_yaml(&PatternEntry {
                        kind,
                        pattern: entry.value.clone(),
                        catalog: Some(catalog.clone()),
                    });
                    let place = entry
                        .line
                        .map_or_else(String::new, |line| format!("line {line}: "));

                    format!("{place}`{}` becomes `{yaml}`", entry.value)
                })
                .collect(),
        };

        let mut detail = vec![format!(
            "a project selects by pattern now: one `{REQUIRES_KEY}:` list, each entry one key naming a namespace and qualified with a `catalogs:` alias"
        )];

        detail.extend(rewrites);

        if removed.kind.is_none() {
            detail.push(
                "a scope reached items across every namespace at once, which one entry does not: declare a pack in the catalog that requires them, and select it with `pack:`".to_owned(),
            );
        }

        detail.push(if catalog == ALIAS_PLACEHOLDER {
            format!(
                "rename the key to `{REQUIRES_KEY}`, qualifying each entry with the alias it should select from"
            )
        } else {
            format!("rename the key to `{REQUIRES_KEY}`")
        });

        return Err(root.key_error(
            removed.key,
            &format!("top-level `{}` is gone", removed.key),
            detail,
        ));
    }

    Ok(())
}

/// Records the names one config list has used, rejecting a repeat and naming both lines.
///
/// Every list in `ambit.yml` is keyed by a name, and every later stage looks each name up exactly
/// once, so a repeat is always a mistake rather than a merge. Refusing it here is what lets
/// resolution treat the lists as maps.
struct NameTracker<'a> {
    file: &'a str,
    /// How the list's entries are named in the message.
    subject: &'a str,
    /// The concrete next step.
    advice: &'a str,
    seen: IndexMap<String, Option<usize>>,
}

impl NameTracker<'_> {
    /// Records `name`, refusing its second use.
    fn track(&mut self, name: &str, line: Option<usize>) -> Result<()> {
        if let Some(first) = self.seen.get(name) {
            return Err(config_error(
                format!(
                    "duplicate {} \"{name}\" {}",
                    self.subject,
                    at(self.file, line)
                ),
                [
                    first.map_or_else(
                        || "already declared earlier".to_owned(),
                        |first| format!("first declared on line {first}"),
                    ),
                    self.advice.to_owned(),
                ],
            ));
        }

        self.seen.insert(name.to_owned(), line);
        Ok(())
    }
}

/// Refuses a catalog alias holding the one character that separates an alias from a pattern.
///
/// An alias is the qualifier half of `<catalog>/<pattern>`, so an alias holding a `/` cannot
/// appear in an address at all: every entry qualified with it would read as a second separator.
/// The alias is refused where it is written, since that is the only place a rename can happen.
///
/// The separator is the only character an alias may not hold. A dot is fine (the reason the
/// separator is `/` and not `.`), and so is a `*`, matched literally, because a qualifier is an
/// alias rather than a pattern.
fn assert_addressable_alias(entry: &YamlMapping, name: &str) -> Result<()> {
    if !name.contains(CATALOG_SEPARATOR) {
        return Ok(());
    }

    Err(entry.key_error(
        "name",
        &format!("catalog name \"{name}\" holds a `{CATALOG_SEPARATOR}`"),
        vec![
            format!(
                "a `{REQUIRES_KEY}` entry addresses an item as `<catalog>{CATALOG_SEPARATOR}<pattern>`, so nothing can select from an alias holding one"
            ),
            format!("rename the catalog to something without a `{CATALOG_SEPARATOR}` — a dot is fine"),
        ],
    ))
}

/// A catalog's `path`, normalized, or `None` when absent or naming the source's root.
///
/// Only a relative path that stays inside the source is accepted. The check is lexical; a symlink
/// leading out of the source is caught once the source is on disk (see
/// [`resolve_catalog_root`](crate::model::catalog::resolve_catalog_root)).
fn parse_catalog_path(entry: &YamlMapping) -> Result<Option<String>> {
    let Some(written) = entry.optional_string("path")? else {
        return Ok(None);
    };

    if written.starts_with(['/', '\\']) || Path::new(&written).is_absolute() {
        return Err(entry.key_error(
            "path",
            &format!("catalog path \"{written}\" is absolute"),
            vec![
                "`path` names a directory inside the source, relative to its root".to_owned(),
                "drop the leading separator, as `path: plugins/acme`".to_owned(),
            ],
        ));
    }

    let mut parts = Vec::new();

    for part in written.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                return Err(entry.key_error(
                    "path",
                    &format!("catalog path \"{written}\" leaves its source"),
                    vec![
                        "`path` names a directory inside the source, so it cannot hold `..`"
                            .to_owned(),
                        "write the directory relative to the source's root, as `path: plugins/acme`"
                            .to_owned(),
                    ],
                ));
            }
            _ => parts.push(part),
        }
    }

    if parts.is_empty() {
        return Ok(None);
    }

    Ok(Some(parts.join("/")))
}

fn parse_catalogs(root: &YamlMapping) -> Result<Vec<CatalogRef>> {
    let mut tracker = NameTracker {
        file: root.file(),
        subject: "catalog name",
        advice: "give each catalog a distinct name",
        seen: IndexMap::new(),
    };
    let mut catalogs = Vec::new();

    for entry in root.optional_mapping_list("catalogs")?.unwrap_or_default() {
        entry.reject_unknown_keys(CATALOG_KEYS)?;

        let name = entry.require_string("name")?;

        assert_addressable_alias(&entry, &name)?;
        tracker.track(&name, entry.line_of("name"))?;

        let r#ref = entry.optional_string("ref")?;
        let path = parse_catalog_path(&entry)?;

        catalogs.push(CatalogRef {
            name,
            source: entry.require_string("source")?,
            r#ref,
            path,
        });
    }

    Ok(catalogs)
}

/// The project's `requires` list, deduplicated, with each entry's line kept.
///
/// The lines come from a second read of the same key rather than from [`parse_entries`], which
/// returns entries and not positions. Pairing them by index is exact: the parse maps one entry to
/// one item of the sequence, in document order, so item *i* is where entry *i* was written. An
/// entry repeated verbatim keeps the first line, the one a reader scanning downward finds.
fn parse_selection(root: &YamlMapping) -> Result<(Vec<PatternEntry>, IndexMap<String, usize>)> {
    let written = parse_entries(root, Addressing::Qualified)?;
    // Every item is a mapping by now: `parse_entries` refuses a bare pattern before returning.
    let items = root.optional_entry_list(REQUIRES_KEY)?.unwrap_or_default();
    let mut lines = IndexMap::new();

    for (entry, item) in written.iter().zip(&items) {
        let line = match item {
            YamlEntry::Mapping(mapping) => mapping.line(),
            YamlEntry::String(_) => None,
        };

        if let Some(line) = line {
            lines.entry(entry_yaml(entry)).or_insert(line);
        }
    }

    Ok((unique_entries(&written), lines))
}

/// Validates a config mapping, whatever it was read from.
fn from_mapping(root: &YamlMapping) -> Result<ProjectConfig> {
    assert_no_inline_definitions(root)?;
    assert_no_removed_selection(root)?;
    root.reject_unknown_keys(CONFIG_KEYS)?;

    let version = root.require_integer("version")?;

    if version != CONFIG_VERSION {
        return Err(root.key_error(
            "version",
            &format!("unsupported config version {version}"),
            vec![
                format!("this build of ambit understands version {CONFIG_VERSION}"),
                format!("set `version: {CONFIG_VERSION}`, or upgrade ambit"),
            ],
        ));
    }

    // Read in the order the keys are documented: a config with two problems should report the
    // earlier key's.
    let harnesses = root.optional_string_list("harnesses")?.unwrap_or_else(|| {
        DEFAULT_HARNESSES
            .iter()
            .map(|&name| name.to_owned())
            .collect()
    });
    let catalogs = parse_catalogs(root)?;
    let (requires, entry_lines) = parse_selection(root)?;

    Ok(ProjectConfig {
        version,
        origin: ConfigOrigin {
            file: root.file().to_owned(),
            entry_lines,
        },
        harnesses,
        catalogs,
        requires,
    })
}

/// Parses an `ambit.yml` document. `file` is how it is named in error messages, conventionally
/// project-relative.
///
/// # Errors
///
/// Exit 2 for anything malformed.
#[cfg(test)]
pub fn parse_project_config(text: &str, file: &str) -> Result<ProjectConfig> {
    from_mapping(&crate::model::yaml::parse_yaml_mapping(text, file)?)
}

/// Whether `target` is a regular file (following symlinks). Any failure to stat it counts as no.
fn is_file(target: &Path) -> bool {
    std::fs::metadata(target).is_ok_and(|metadata| metadata.is_file())
}

/// Which accepted config filenames `project_dir` already holds, in preference order.
///
/// Shared with `ambit init`, whose question is the opposite of [`find_config_file`]'s: it must
/// refuse a directory that holds either name, naming the file it found rather than the one it was
/// about to write.
///
/// # Errors
///
/// Never in practice: a name that cannot be inspected counts as absent. The `Result` is kept for
/// the callers' signatures.
#[allow(clippy::unnecessary_wraps)]
pub fn existing_config_files(project_dir: &Path) -> Result<Vec<String>> {
    Ok(CONFIG_FILENAMES
        .iter()
        .filter(|name| is_file(&join(project_dir, name)))
        .map(|&name| name.to_owned())
        .collect())
}

/// Finds the config file in `project_dir`.
///
/// # Errors
///
/// Exit 2 if there is no config, or more than one.
pub fn find_config_file(project_dir: &Path) -> Result<FoundConfig> {
    let present = existing_config_files(project_dir)?;
    let shown = project_dir.display();

    match present.as_slice() {
        [] => Err(config_error(
            format!("no ambit config in {shown}"),
            [
                format!("expected one of: {}", CONFIG_FILENAMES.join(", ")),
                "run `ambit init` to scaffold one".to_owned(),
            ],
        )),
        [file] => Ok(FoundConfig {
            path: join(project_dir, file),
            file: file.clone(),
        }),
        _ => Err(config_error(
            format!("{} both exist in {shown}", present.join(" and ")),
            [
                "ambit cannot tell which one is authoritative".to_owned(),
                format!("delete one, keeping {}", CONFIG_FILENAMES[0]),
            ],
        )),
    }
}

/// Loads the config for a project directory.
///
/// # Errors
///
/// Exit 2 if the config is missing, ambiguous, or malformed.
pub fn load_project_config(project_dir: &Path) -> Result<ProjectConfig> {
    let found = find_config_file(project_dir)?;

    from_mapping(&read_yaml_mapping(&found.path, &found.file)?)
}

#[cfg(test)]
mod tests;
