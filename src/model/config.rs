use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::{Result, at, config_error};
use crate::model::pattern::{
    Addressing, PatternEntry, REQUIRES_KEY, entry_yaml, parse_entries, unique_entries,
};
use crate::model::requirement::{CATALOG_SEPARATOR, ItemKind};
use crate::model::sources::is_path_source;
use crate::model::yaml::{YamlEntry, YamlMapping, read_yaml_mapping};
use crate::util::path::join;
use crate::util::string_enum;

pub const CONFIG_VERSION: i64 = 1;

pub const DEFAULT_HARNESSES: &[&str] = &["claude"];

pub const CONFIG_FILENAMES: [&str; 2] = ["ambit.yml", "ambit.yaml"];

string_enum! {
    pub enum Trust {
        Full => "full",
        Review => "review",
    }
}

impl Trust {
    pub fn default_for(source: &str) -> Self {
        if is_path_source(source) {
            Self::Full
        } else {
            Self::Review
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRef {
    pub name: String,
    pub source: String,
    pub r#ref: Option<String>,
    pub path: Option<String>,
    pub trust: Trust,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigOrigin {
    pub file: String,
    pub entry_lines: IndexMap<String, usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectConfig {
    pub version: i64,
    pub origin: ConfigOrigin,
    pub harnesses: Vec<String>,
    pub catalogs: Vec<CatalogRef>,
    pub requires: Vec<PatternEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundConfig {
    pub path: PathBuf,
    pub file: String,
}

const CONFIG_KEYS: &[&str] = &["catalogs", "harnesses", REQUIRES_KEY, "version"];
const CATALOG_KEYS: &[&str] = &["name", "path", "ref", "source", "trust"];

const SELF_CATALOG_ADVICE: &str =
    "then list this project as a catalog: `- name: local` with `source: path:.`";

struct RemovedInlineKey {
    key: &'static str,
    subject: &'static str,
    file: &'static str,
}

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

struct RemovedSelectionKey {
    key: &'static str,
    kind: Option<ItemKind>,
}

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

const ALIAS_PLACEHOLDER: &str = "<catalog>";

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

fn rewrite_alias(root: &YamlMapping) -> String {
    match catalog_aliases(root).as_slice() {
        [only] => only.clone(),
        _ => ALIAS_PLACEHOLDER.to_owned(),
    }
}

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

struct NameTracker<'a> {
    file: &'a str,
    subject: &'a str,
    advice: &'a str,
    seen: IndexMap<String, Option<usize>>,
}

impl NameTracker<'_> {
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

fn parse_trust(entry: &YamlMapping, source: &str) -> Result<Trust> {
    let Some(written) = entry.optional_string("trust")? else {
        return Ok(Trust::default_for(source));
    };

    Trust::parse(&written).ok_or_else(|| {
        let known: Vec<&str> = Trust::ALL.iter().map(|trust| trust.as_str()).collect();

        entry.key_error(
            "trust",
            &format!("unknown trust \"{written}\""),
            vec![
                format!("`trust` is one of: {}", known.join(", ")),
                "write `trust: review` to gate new execution from this catalog, or `trust: full` to install it as is".to_owned(),
            ],
        )
    })
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
        let source = entry.require_string("source")?;
        let trust = parse_trust(&entry, &source)?;

        catalogs.push(CatalogRef {
            name,
            source,
            r#ref,
            path,
            trust,
        });
    }

    Ok(catalogs)
}

fn parse_selection(root: &YamlMapping) -> Result<(Vec<PatternEntry>, IndexMap<String, usize>)> {
    let written = parse_entries(root, Addressing::Qualified)?;
    // Zipped by index below: `parse_entries` yields one entry per item, in document order.
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

fn from_mapping(root: &YamlMapping) -> Result<ProjectConfig> {
    // Before `reject_unknown_keys`, so these specific refusals win over its generic one.
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

    // In documented key order: with two problems, the earlier key's is reported.
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

#[cfg(test)]
pub fn parse_project_config(text: &str, file: &str) -> Result<ProjectConfig> {
    from_mapping(&crate::model::yaml::parse_yaml_mapping(text, file)?)
}

fn is_file(target: &Path) -> bool {
    std::fs::metadata(target).is_ok_and(|metadata| metadata.is_file())
}

#[allow(clippy::unnecessary_wraps)]
pub fn existing_config_files(project_dir: &Path) -> Result<Vec<String>> {
    Ok(CONFIG_FILENAMES
        .iter()
        .filter(|name| is_file(&join(project_dir, name)))
        .map(|&name| name.to_owned())
        .collect())
}

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

pub fn load_project_config(project_dir: &Path) -> Result<ProjectConfig> {
    let found = find_config_file(project_dir)?;

    from_mapping(&read_yaml_mapping(&found.path, &found.file)?)
}

#[cfg(test)]
mod tests;
