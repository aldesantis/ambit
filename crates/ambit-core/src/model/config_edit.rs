//! Edits to an `ambit.yml` that keep the rest of the file as its author wrote it.
//!
//! Each edit is spliced into the text by [`YamlEditor`], then the result is parsed again and
//! compared with the config the edit should produce. A mismatch is a bug in the editor and fails
//! with exit 1 rather than returning text that says something other than what was asked.
//!
//! Edits address what the config means, not where it is written: a catalog by name, a selection by
//! its namespace, catalog and pattern. A selection written twice is one selection, so removing or
//! replacing it acts on every copy.

use crate::errors::{AmbitError, Result, config_error};
use crate::model::config::{
    CONFIG_VERSION, CatalogRef, DEFAULT_HARNESSES, ProjectConfig, parse_project_config,
};
use crate::model::pattern::{PatternEntry, REQUIRES_KEY, entry_address, format_entry};
use crate::model::requirement::{CATALOG_SEPARATOR, ItemKind};
use crate::model::yaml::edit::{
    Patch, PlannedItem, YamlEditor, YamlNode, plain_scalar, quoted_scalar,
};
use crate::util::text::js_trim;

const HARNESSES_KEY: &str = "harnesses";
const CATALOGS_KEY: &str = "catalogs";

/// One change to a config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigEdit {
    /// Replaces the agent tools, in this order. Duplicates are dropped.
    SetHarnesses {
        harnesses: Vec<String>,
    },
    AddCatalog {
        name: String,
        source: String,
        r#ref: Option<String>,
    },
    /// Points an existing catalog somewhere else. Selections from it are kept; whether they still
    /// resolve is for resolution to judge.
    SetCatalogSource {
        name: String,
        source: String,
        r#ref: Option<String>,
    },
    /// Renames a catalog and rewrites every selection qualified with the old name, keeping each
    /// selection's namespace and pattern.
    RenameCatalog {
        from: String,
        to: String,
    },
    /// Removes a catalog together with every selection qualified with its name.
    RemoveCatalog {
        name: String,
    },
    /// Adds a selection: one item or, with a `*` in its pattern, a rule.
    AddEntry {
        entry: PatternEntry,
    },
    RemoveEntry {
        entry: PatternEntry,
    },
    /// Swaps one selection for another in place.
    ReplaceEntry {
        old: PatternEntry,
        new: PatternEntry,
    },
}

/// How one config differs from another; see [`config_changes`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConfigChanges {
    pub harnesses_added: Vec<String>,
    pub harnesses_removed: Vec<String>,
    pub catalogs_added: Vec<CatalogRef>,
    pub catalogs_removed: Vec<CatalogRef>,
    /// Catalogs kept under the same name whose source or ref changed, as `(before, after)`.
    pub catalogs_changed: Vec<(CatalogRef, CatalogRef)>,
    /// `(from, to)` names.
    pub catalogs_renamed: Vec<(String, String)>,
    pub entries_added: Vec<PatternEntry>,
    pub entries_removed: Vec<PatternEntry>,
}

impl ConfigChanges {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// The config a new setup starts from: the current version, `harnesses`, and empty `catalogs` and
/// `requires` lists.
pub fn new_config_text(harnesses: &[String]) -> String {
    let harnesses = if harnesses.is_empty() {
        format!("{HARNESSES_KEY}: []\n")
    } else {
        let mut text = format!("{HARNESSES_KEY}:\n");

        for harness in unique(harnesses) {
            text.push_str("  - ");
            text.push_str(&plain_scalar(&harness));
            text.push('\n');
        }

        text
    };

    format!("version: {CONFIG_VERSION}\n{harnesses}{CATALOGS_KEY}: []\n{REQUIRES_KEY}: []\n")
}

/// Applies `edits` to the config `text`, in order, keeping every byte outside the edited nodes.
///
/// `file` is how the config is named in errors: `ambit.yml` or `ambit.yaml`. Each edit sees the
/// result of the one before it.
///
/// # Errors
///
/// Exit 2 when `text` is not a valid config, when an edit names a catalog or selection the config
/// does not have, adds one it already has, or carries an invalid name or value, and when the
/// edited node uses an anchor or a tag or a layout the editor does not splice. Exit 1 when the
/// edited text does not read back as the intended config.
pub fn edit_config_text(text: &str, file: &str, edits: &[ConfigEdit]) -> Result<String> {
    let mut text = text.to_owned();

    for edit in edits {
        text = apply(&text, file, edit)?;
    }

    Ok(text)
}

/// The selections qualified with `catalog`, in the order the config lists them.
pub fn catalog_references(config: &ProjectConfig, catalog: &str) -> Vec<PatternEntry> {
    config
        .requires
        .iter()
        .filter(|entry| entry.catalog.as_deref() == Some(catalog))
        .cloned()
        .collect()
}

/// Checks `name` as a catalog name for `config`, under the parser's rules: not empty, no
/// [`CATALOG_SEPARATOR`], and not already taken. `renaming` is the catalog being renamed, whose
/// own name does not count as taken.
///
/// # Errors
///
/// Exit 2 naming the rule `name` breaks.
pub fn validate_catalog_name(
    config: &ProjectConfig,
    name: &str,
    renaming: Option<&str>,
) -> Result<()> {
    check_text("catalog name", name)?;

    if name.contains(CATALOG_SEPARATOR) {
        return Err(config_error(
            format!("catalog name \"{name}\" holds a `{CATALOG_SEPARATOR}`"),
            [
                format!(
                    "a `{REQUIRES_KEY}` entry addresses an item as `<catalog>{CATALOG_SEPARATOR}<pattern>`, so nothing can select from a name holding one"
                ),
                format!("choose a name without a `{CATALOG_SEPARATOR}`; a dot is fine"),
            ],
        ));
    }

    let taken = config
        .catalogs
        .iter()
        .any(|catalog| catalog.name == name && Some(name) != renaming);

    if taken {
        return Err(config_error(
            format!("duplicate catalog name \"{name}\" ({})", config.origin.file),
            ["give each catalog a distinct name"],
        ));
    }

    Ok(())
}

/// A catalog name for `source`: the repository's or folder's basename, without `.git`.
///
/// A suggestion only. It is not checked against the names a config already uses; see
/// [`validate_catalog_name`].
pub fn propose_catalog_name(source: &str) -> String {
    let source = js_trim(source);
    let location = source
        .strip_prefix("path:")
        .or_else(|| source.strip_prefix("git:"))
        .unwrap_or(source);
    // An `owner/repo@ref` shorthand carries its ref after the last `@`. A URL's or an ssh
    // remote's `@` precedes the host instead.
    let location = match location.rfind('@') {
        Some(at) if !location.contains("://") && location[..at].contains('/') => &location[..at],
        _ => location,
    };
    let trimmed = location.trim_end_matches(['/', '\\']);
    let base = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or(trimmed);
    let base = base.strip_suffix(".git").unwrap_or(base);

    if base.is_empty() || base == "." || base == ".." {
        "catalog".to_owned()
    } else {
        base.to_owned()
    }
}

/// How `draft` differs from `base`. With no base, everything in the draft counts as added.
///
/// A catalog removed from `base` and one added to `draft` with the same source and ref count as a
/// rename when that pairing is unambiguous. Selections are then compared with the old name
/// rewritten to the new one, so a rename alone adds and removes no selection.
pub fn config_changes(base: Option<&ProjectConfig>, draft: &ProjectConfig) -> ConfigChanges {
    let empty = Vec::new();
    let (base_harnesses, base_catalogs, base_entries) = match base {
        Some(base) => (&base.harnesses, &base.catalogs, &base.requires),
        None => (&empty, &Vec::new(), &Vec::new()),
    };
    let named = |catalogs: &[CatalogRef], name: &str| -> Option<CatalogRef> {
        catalogs
            .iter()
            .find(|catalog| catalog.name == name)
            .cloned()
    };

    let mut changes = ConfigChanges {
        harnesses_added: difference(&draft.harnesses, base_harnesses),
        harnesses_removed: difference(base_harnesses, &draft.harnesses),
        ..ConfigChanges::default()
    };

    let mut removed: Vec<CatalogRef> = base_catalogs
        .iter()
        .filter(|catalog| named(&draft.catalogs, &catalog.name).is_none())
        .cloned()
        .collect();
    let mut added: Vec<CatalogRef> = draft
        .catalogs
        .iter()
        .filter(|catalog| named(base_catalogs, &catalog.name).is_none())
        .cloned()
        .collect();

    for before in base_catalogs {
        if let Some(after) = named(&draft.catalogs, &before.name)
            && &after != before
        {
            changes.catalogs_changed.push((before.clone(), after));
        }
    }

    let same_place = |a: &CatalogRef, b: &CatalogRef| a.source == b.source && a.r#ref == b.r#ref;
    let renames: Vec<(CatalogRef, CatalogRef)> = removed
        .iter()
        .filter_map(|old| {
            let mut targets = added.iter().filter(|new| same_place(old, new));
            let new = targets.next()?;
            let sources = removed
                .iter()
                .filter(|other| same_place(other, new))
                .count();

            (targets.next().is_none() && sources == 1).then(|| (old.clone(), new.clone()))
        })
        .collect();

    removed.retain(|old| !renames.iter().any(|(from, _)| from == old));
    added.retain(|new| !renames.iter().any(|(_, to)| to == new));
    changes.catalogs_removed = removed;
    changes.catalogs_added = added;
    changes.catalogs_renamed = renames
        .iter()
        .map(|(from, to)| (from.name.clone(), to.name.clone()))
        .collect();

    let translated: Vec<PatternEntry> = base_entries
        .iter()
        .map(|entry| {
            let renamed = renames
                .iter()
                .find(|(from, _)| entry.catalog.as_deref() == Some(from.name.as_str()));

            match renamed {
                Some((_, to)) => with_catalog(entry, &to.name),
                None => entry.clone(),
            }
        })
        .collect();

    changes.entries_added = difference(&draft.requires, &translated);
    changes.entries_removed = base_entries
        .iter()
        .zip(&translated)
        .filter(|(_, translated)| !draft.requires.contains(translated))
        .map(|(entry, _)| entry.clone())
        .collect();

    changes
}

/// The items of `a` that `b` lacks, in `a`'s order.
fn difference<T: Clone + PartialEq>(a: &[T], b: &[T]) -> Vec<T> {
    a.iter().filter(|item| !b.contains(item)).cloned().collect()
}

fn unique(values: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();

    for value in values {
        if !kept.contains(value) {
            kept.push(value.clone());
        }
    }

    kept
}

fn with_catalog(entry: &PatternEntry, catalog: &str) -> PatternEntry {
    PatternEntry {
        catalog: Some(catalog.to_owned()),
        ..entry.clone()
    }
}

/// What a config says, without where it says it.
#[derive(Debug, PartialEq, Eq)]
struct Meaning {
    harnesses: Vec<String>,
    catalogs: Vec<CatalogRef>,
    requires: Vec<PatternEntry>,
}

impl Meaning {
    fn of(config: &ProjectConfig) -> Self {
        Self {
            harnesses: config.harnesses.clone(),
            catalogs: config.catalogs.clone(),
            requires: config.requires.clone(),
        }
    }
}

/// Applies one edit, then checks the result reads back as intended.
fn apply(text: &str, file: &str, edit: &ConfigEdit) -> Result<String> {
    let config = parse_project_config(text, file)?;
    let mut editor = YamlEditor::parse(text, file)?;
    let mut expected = Meaning::of(&config);

    match edit {
        ConfigEdit::SetHarnesses { harnesses } => {
            set_harnesses(&mut editor, harnesses, &mut expected)?;
        }
        ConfigEdit::AddCatalog {
            name,
            source,
            r#ref,
        } => {
            let catalog = new_catalog(name, source, r#ref.as_deref())?;

            validate_catalog_name(&config, name, None)?;

            let mut plan = keep_catalogs(&editor);

            plan.push(PlannedItem::added(catalog_lines(&catalog)));
            update_list(&mut editor, CATALOGS_KEY, &plan)?;
            expected.catalogs.push(catalog);
        }
        ConfigEdit::SetCatalogSource {
            name,
            source,
            r#ref,
        } => {
            let catalog = new_catalog(name, source, r#ref.as_deref())?;

            set_catalog_source(&mut editor, &config, &catalog, &mut expected)?;
        }
        ConfigEdit::RenameCatalog { from, to } => {
            find_catalog(&config, from)?;
            validate_catalog_name(&config, to, Some(from))?;
            rename_catalog(&mut editor, from, to, &mut expected)?;
        }
        ConfigEdit::RemoveCatalog { name } => {
            find_catalog(&config, name)?;
            remove_catalog(&mut editor, name, &mut expected)?;
        }
        ConfigEdit::AddEntry { entry } => {
            check_entry(entry)?;

            if config.requires.contains(entry) {
                return Err(config_error(
                    format!("`{}` is already selected ({file})", format_entry(entry)),
                    ["each selection is listed once"],
                ));
            }

            let mut plan = keep_entries(&editor);

            plan.push(PlannedItem::added(entry_lines(entry)));
            update_list(&mut editor, REQUIRES_KEY, &plan)?;
            expected.requires.push(entry.clone());
        }
        ConfigEdit::RemoveEntry { entry } => {
            find_entry(&config, entry)?;
            replace_entry(&mut editor, entry, None)?;
            expected.requires.retain(|kept| kept != entry);
        }
        ConfigEdit::ReplaceEntry { old, new } => {
            find_entry(&config, old)?;
            check_entry(new)?;

            if old != new {
                if config.requires.contains(new) {
                    return Err(config_error(
                        format!("`{}` is already selected ({file})", format_entry(new)),
                        [format!(
                            "remove `{}` instead of replacing it",
                            format_entry(old)
                        )],
                    ));
                }

                replace_entry(&mut editor, old, Some(new))?;

                for entry in &mut expected.requires {
                    if entry == old {
                        entry.clone_from(new);
                    }
                }
            }
        }
    }

    let edited = editor.finish()?;

    verify(&edited, file, &expected)?;
    Ok(edited)
}

/// Parses the edited text and compares what it says with what the edit meant it to say.
fn verify(edited: &str, file: &str, expected: &Meaning) -> Result<()> {
    let reparsed = parse_project_config(edited, file).map_err(|error| {
        AmbitError::unexpected(format!(
            "the edited {file} does not parse: {}",
            error.message
        ))
    })?;

    if &Meaning::of(&reparsed) != expected {
        return Err(AmbitError::unexpected(format!(
            "the edited {file} does not read back as the intended config"
        )));
    }

    Ok(())
}

fn set_harnesses(
    editor: &mut YamlEditor,
    harnesses: &[String],
    expected: &mut Meaning,
) -> Result<()> {
    for harness in harnesses {
        check_text("agent tool", harness)?;
    }

    let desired = unique(harnesses);

    // An absent key already means the default, so writing it out would be a change to the file
    // that changes nothing.
    if editor.root_value(HARNESSES_KEY).is_none() && desired == DEFAULT_HARNESSES {
        return Ok(());
    }

    let written: Vec<(YamlNode, String)> = editor
        .root_value(HARNESSES_KEY)
        .map(|seq| editor.items(seq))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| Some((item, editor.string(item)?.to_owned())))
        .collect();
    let plan: Vec<PlannedItem> = desired
        .iter()
        .map(|harness| {
            let lines = vec![plain_scalar(harness)];

            match written.iter().find(|(_, value)| value == harness) {
                Some(&(node, _)) => PlannedItem::keep(node, lines),
                None => PlannedItem::added(lines),
            }
        })
        .collect();

    editor.update_sequence(HARNESSES_KEY, &plan)?;
    expected.harnesses = desired;
    Ok(())
}

fn set_catalog_source(
    editor: &mut YamlEditor,
    config: &ProjectConfig,
    catalog: &CatalogRef,
    expected: &mut Meaning,
) -> Result<()> {
    let current = find_catalog(config, &catalog.name)?;

    if current == catalog {
        return Ok(());
    }

    let plan: Vec<PlannedItem> = written_catalogs(editor)
        .into_iter()
        .map(|(node, written)| {
            if written.name != catalog.name {
                return PlannedItem::keep(node, catalog_lines(&written));
            }

            let mut patches = Vec::new();

            if written.source != catalog.source
                && let Some(source) = editor.value(node, "source")
            {
                patches.push(Patch::Scalar {
                    node: source,
                    value: catalog.source.clone(),
                });
            }

            match (&catalog.r#ref, editor.value(node, "ref")) {
                (Some(new), Some(old)) if written.r#ref.as_ref() != Some(new) => {
                    patches.push(Patch::Scalar {
                        node: old,
                        value: new.clone(),
                    });
                }
                (Some(new), None) => patches.push(Patch::InsertKey {
                    key: "ref".to_owned(),
                    value: new.clone(),
                }),
                (None, Some(_)) => patches.push(Patch::RemoveKey {
                    key: "ref".to_owned(),
                }),
                _ => {}
            }

            PlannedItem::patch(node, patches, catalog_lines(catalog))
        })
        .collect();

    update_list(editor, CATALOGS_KEY, &plan)?;

    for entry in &mut expected.catalogs {
        if entry.name == catalog.name {
            entry.clone_from(catalog);
        }
    }

    Ok(())
}

fn rename_catalog(
    editor: &mut YamlEditor,
    from: &str,
    to: &str,
    expected: &mut Meaning,
) -> Result<()> {
    if from == to {
        return Ok(());
    }

    let catalogs: Vec<PlannedItem> = written_catalogs(editor)
        .into_iter()
        .map(|(node, written)| {
            if written.name != from {
                return PlannedItem::keep(node, catalog_lines(&written));
            }

            let renamed = CatalogRef {
                name: to.to_owned(),
                ..written
            };
            let patches = editor
                .value(node, "name")
                .map(|name| Patch::Scalar {
                    node: name,
                    value: to.to_owned(),
                })
                .into_iter()
                .collect();

            PlannedItem::patch(node, patches, catalog_lines(&renamed))
        })
        .collect();
    let entries: Vec<PlannedItem> = written_entries(editor)
        .into_iter()
        .map(|(node, entry)| {
            if entry.catalog.as_deref() != Some(from) {
                return PlannedItem::keep(node, entry_lines(&entry));
            }

            let renamed = with_catalog(&entry, to);

            address_patch(editor, node, &renamed)
        })
        .collect();

    update_list(editor, CATALOGS_KEY, &catalogs)?;
    update_list(editor, REQUIRES_KEY, &entries)?;

    for catalog in &mut expected.catalogs {
        if catalog.name == from {
            to.clone_into(&mut catalog.name);
        }
    }

    for entry in &mut expected.requires {
        if entry.catalog.as_deref() == Some(from) {
            *entry = with_catalog(entry, to);
        }
    }

    Ok(())
}

fn remove_catalog(editor: &mut YamlEditor, name: &str, expected: &mut Meaning) -> Result<()> {
    let catalogs: Vec<PlannedItem> = written_catalogs(editor)
        .into_iter()
        .filter(|(_, written)| written.name != name)
        .map(|(node, written)| PlannedItem::keep(node, catalog_lines(&written)))
        .collect();
    let entries: Vec<PlannedItem> = written_entries(editor)
        .into_iter()
        .filter(|(_, entry)| entry.catalog.as_deref() != Some(name))
        .map(|(node, entry)| PlannedItem::keep(node, entry_lines(&entry)))
        .collect();

    update_list(editor, CATALOGS_KEY, &catalogs)?;
    update_list(editor, REQUIRES_KEY, &entries)?;
    expected.catalogs.retain(|catalog| catalog.name != name);
    expected
        .requires
        .retain(|entry| entry.catalog.as_deref() != Some(name));
    Ok(())
}

/// Replaces the first written copy of `old` with `new`, or removes it when `new` is absent. Every
/// later copy of `old` is removed: it was the same selection.
fn replace_entry(
    editor: &mut YamlEditor,
    old: &PatternEntry,
    new: Option<&PatternEntry>,
) -> Result<()> {
    let mut replaced = new.is_none();
    let plan: Vec<PlannedItem> = written_entries(editor)
        .into_iter()
        .filter_map(|(node, entry)| {
            if &entry != old {
                return Some(PlannedItem::keep(node, entry_lines(&entry)));
            }

            if replaced {
                return None;
            }

            replaced = true;
            let new = new?;

            Some(if new.kind == old.kind {
                address_patch(editor, node, new)
            } else {
                PlannedItem::replace(node, entry_lines(new))
            })
        })
        .collect();

    update_list(editor, REQUIRES_KEY, &plan)
}

/// A kept selection whose address changes, its namespace key and the quoting of its value kept.
fn address_patch(editor: &YamlEditor, node: YamlNode, entry: &PatternEntry) -> PlannedItem {
    let patches = editor
        .value(node, entry.kind.as_str())
        .map(|value| Patch::Scalar {
            node: value,
            value: entry_address(entry),
        })
        .into_iter()
        .collect();

    PlannedItem::patch(node, patches, entry_lines(entry))
}

/// The catalogs as written, each with its item node. The config was already validated, so every
/// item is a mapping with a `name` and a `source`.
fn written_catalogs(editor: &YamlEditor) -> Vec<(YamlNode, CatalogRef)> {
    let field = |node: YamlNode, key: &str| -> Option<String> {
        editor
            .value(node, key)
            .and_then(|value| editor.string(value))
            .map(str::to_owned)
    };

    editor
        .root_value(CATALOGS_KEY)
        .map(|seq| editor.items(seq))
        .unwrap_or_default()
        .into_iter()
        .map(|node| {
            let catalog = CatalogRef {
                name: field(node, "name").unwrap_or_default(),
                source: field(node, "source").unwrap_or_default(),
                r#ref: field(node, "ref"),
            };

            (node, catalog)
        })
        .collect()
}

/// The `requires` entries as written, duplicates included, each with its item node. The config
/// was already validated, so every item is a one-key mapping holding a qualified address.
fn written_entries(editor: &YamlEditor) -> Vec<(YamlNode, PatternEntry)> {
    editor
        .root_value(REQUIRES_KEY)
        .map(|seq| editor.items(seq))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|node| {
            let kind = editor.keys(node).into_iter().find_map(ItemKind::parse)?;
            let address = editor.string(editor.value(node, kind.as_str())?)?;
            let (catalog, pattern) = address.split_once(CATALOG_SEPARATOR)?;

            Some((
                node,
                PatternEntry {
                    kind,
                    pattern: pattern.to_owned(),
                    catalog: Some(catalog.to_owned()),
                },
            ))
        })
        .collect()
}

/// Rewrites the list under `key` to `plan`. An absent `catalogs` or `requires` already means an
/// empty list, so emptying one is no change.
fn update_list(editor: &mut YamlEditor, key: &str, plan: &[PlannedItem]) -> Result<()> {
    if plan.is_empty() && editor.root_value(key).is_none() {
        return Ok(());
    }

    editor.update_sequence(key, plan)
}

fn keep_catalogs(editor: &YamlEditor) -> Vec<PlannedItem> {
    written_catalogs(editor)
        .into_iter()
        .map(|(node, catalog)| PlannedItem::keep(node, catalog_lines(&catalog)))
        .collect()
}

fn keep_entries(editor: &YamlEditor) -> Vec<PlannedItem> {
    written_entries(editor)
        .into_iter()
        .map(|(node, entry)| PlannedItem::keep(node, entry_lines(&entry)))
        .collect()
}

fn catalog_lines(catalog: &CatalogRef) -> Vec<String> {
    let mut lines = vec![
        format!("name: {}", plain_scalar(&catalog.name)),
        format!("source: {}", plain_scalar(&catalog.source)),
    ];

    if let Some(r#ref) = &catalog.r#ref {
        lines.push(format!("ref: {}", plain_scalar(r#ref)));
    }

    lines
}

/// A selection as one block line. The address is always quoted, as
/// [`entry_yaml`](crate::model::pattern::entry_yaml) writes it.
fn entry_lines(entry: &PatternEntry) -> Vec<String> {
    vec![format!(
        "{}: {}",
        entry.kind,
        quoted_scalar(&entry_address(entry))
    )]
}

fn find_catalog<'a>(config: &'a ProjectConfig, name: &str) -> Result<&'a CatalogRef> {
    config
        .catalogs
        .iter()
        .find(|catalog| catalog.name == name)
        .ok_or_else(|| {
            config_error(
                format!("no catalog named \"{name}\" ({})", config.origin.file),
                ["reload the setup and pick a catalog it lists"],
            )
        })
}

fn find_entry(config: &ProjectConfig, entry: &PatternEntry) -> Result<()> {
    if config.requires.contains(entry) {
        return Ok(());
    }

    Err(config_error(
        format!(
            "`{}` is not selected ({})",
            format_entry(entry),
            config.origin.file
        ),
        ["reload the setup and pick a selection it lists"],
    ))
}

fn new_catalog(name: &str, source: &str, r#ref: Option<&str>) -> Result<CatalogRef> {
    check_text("catalog source", source)?;

    if let Some(r#ref) = r#ref {
        check_text("catalog revision", r#ref)?;
    }

    Ok(CatalogRef {
        name: name.to_owned(),
        source: source.to_owned(),
        r#ref: r#ref.map(str::to_owned),
    })
}

/// Checks a selection is one a project config can hold: qualified, with a catalog and a pattern
/// that are each one non-empty line holding no [`CATALOG_SEPARATOR`].
fn check_entry(entry: &PatternEntry) -> Result<()> {
    let Some(catalog) = &entry.catalog else {
        return Err(config_error(
            format!("selection `{}` names no catalog", format_entry(entry)),
            [format!(
                "a project selects as `<catalog>{CATALOG_SEPARATOR}<pattern>`"
            )],
        ));
    };

    for (what, value) in [("catalog", catalog), ("pattern", &entry.pattern)] {
        check_text(&format!("selection {what}"), value)?;

        if value.contains(CATALOG_SEPARATOR) {
            return Err(config_error(
                format!("selection {what} \"{value}\" holds a `{CATALOG_SEPARATOR}`"),
                [format!(
                    "a selection is written `<catalog>{CATALOG_SEPARATOR}<pattern>`, with one `{CATALOG_SEPARATOR}`"
                )],
            ));
        }
    }

    Ok(())
}

/// Refuses an empty value, or one holding a line break or another control character, which would
/// not survive as one line of the config.
fn check_text(what: &str, value: &str) -> Result<()> {
    if js_trim(value).is_empty() {
        return Err(config_error(
            format!("{what} must not be empty"),
            [format!("give the {what} a value")],
        ));
    }

    if value.chars().any(char::is_control) {
        return Err(config_error(
            format!(
                "{what} \"{}\" holds a control character",
                value.escape_debug()
            ),
            [format!("write the {what} on one line")],
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests;
