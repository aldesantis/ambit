//! Config editing for the app's draft: pure functions over config text, no files touched.
//!
//! The app keeps the base text and the list of edits, and replays the edits from the base on every
//! draft change through [`edit_config`]. Each function takes the text and the file name it would be
//! saved under, so errors name `ambit.yml` or `ambit.yaml` with a line where the parser has one.

use ambit_core::errors::AmbitError;
use ambit_core::model::config::{CatalogRef, parse_project_config};
use ambit_core::model::config_edit as core;
use ambit_core::model::pattern::{Addressing, PatternEntry, parse_address};

use crate::errors::{EngineError, guard};
use crate::records::{CatalogEntry, ConfigSummary, ItemKind, SelectionEntry};

/// One change to a config; see the core's `ConfigEdit` for what each does.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum ConfigEdit {
    /// Replaces the agent tools, in this order. Duplicates are dropped.
    SetHarnesses {
        harnesses: Vec<String>,
    },
    AddCatalog {
        name: String,
        source: String,
        git_ref: Option<String>,
    },
    /// Points an existing catalog somewhere else, keeping its selections.
    SetCatalogSource {
        name: String,
        source: String,
        git_ref: Option<String>,
    },
    /// Renames a catalog and every selection qualified with the old name.
    RenameCatalog {
        from: String,
        to: String,
    },
    /// Removes a catalog and every selection qualified with its name.
    RemoveCatalog {
        name: String,
    },
    AddEntry {
        entry: EntryAddress,
    },
    /// Removes every written copy of the selection.
    RemoveEntry {
        entry: EntryAddress,
    },
    /// Swaps one selection for another in place.
    ReplaceEntry {
        old: EntryAddress,
        new: EntryAddress,
    },
}

/// A selection as the config writes it: a namespace and a `<catalog>/<pattern>` address.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct EntryAddress {
    pub kind: ItemKind,
    pub address: String,
}

/// The text after a batch of edits, and what it now says.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct EditedConfig {
    pub text: String,
    pub summary: ConfigSummary,
}

/// How a draft differs from its base, for the review's config section.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ConfigChanges {
    pub harnesses_added: Vec<String>,
    pub harnesses_removed: Vec<String>,
    pub catalogs_added: Vec<CatalogEntry>,
    pub catalogs_removed: Vec<CatalogEntry>,
    /// Catalogs kept under the same name whose source or ref changed.
    pub catalogs_changed: Vec<CatalogChange>,
    pub catalogs_renamed: Vec<CatalogRename>,
    pub entries_added: Vec<SelectionEntry>,
    pub entries_removed: Vec<SelectionEntry>,
    /// Whether the draft says the same as the base.
    pub is_empty: bool,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CatalogChange {
    pub before: CatalogEntry,
    pub after: CatalogEntry,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CatalogRename {
    pub from: String,
    pub to: String,
}

/// The config a new setup starts from: the current version, `harnesses`, and empty `catalogs` and
/// `requires` lists.
#[uniffi::export]
// UniFFI hands over lists and optional strings by value; it has no borrowed form for them.
#[allow(clippy::needless_pass_by_value)]
pub fn new_config_text(harnesses: Vec<String>) -> String {
    core::new_config_text(&harnesses)
}

/// Applies `edits` to `text` in order, keeping every byte outside the edited nodes.
///
/// # Errors
///
/// [`EngineError::Config`] when `text` is not a valid config, an address does not parse, or an edit
/// names something the config lacks, adds something it has, or touches a node the editor does not
/// splice. [`EngineError::Internal`] when the edited text does not read back as intended.
#[uniffi::export]
pub fn edit_config(
    text: &str,
    file_name: &str,
    edits: Vec<ConfigEdit>,
) -> Result<EditedConfig, EngineError> {
    guard(|| {
        let edits = edits
            .into_iter()
            .map(ConfigEdit::into_core)
            .collect::<Result<Vec<_>, _>>()?;
        let text = core::edit_config_text(text, file_name, &edits)?;
        let summary = ConfigSummary::from(&parse_project_config(&text, file_name)?);

        Ok(EditedConfig { text, summary })
    })
}

/// What `text` says.
///
/// # Errors
///
/// [`EngineError::Config`], with the line, when `text` is not a valid config.
#[uniffi::export]
pub fn parse_config(text: &str, file_name: &str) -> Result<ConfigSummary, EngineError> {
    guard(|| Ok(ConfigSummary::from(&parse_project_config(text, file_name)?)))
}

/// How `draft` differs from `base`. With no base (a setup without a config yet), everything in the
/// draft counts as added.
///
/// # Errors
///
/// [`EngineError::Config`] when either text is not a valid config.
#[uniffi::export]
pub fn config_changes(
    base: Option<String>,
    draft: &str,
    file_name: &str,
) -> Result<ConfigChanges, EngineError> {
    guard(|| {
        let base = base
            .map(|base| parse_project_config(&base, file_name))
            .transpose()?;
        let draft = parse_project_config(draft, file_name)?;

        Ok(ConfigChanges::from(core::config_changes(
            base.as_ref(),
            &draft,
        )))
    })
}

/// The selections qualified with `catalog`, in the order the config lists them: what removing
/// the catalog would also remove.
///
/// # Errors
///
/// [`EngineError::Config`] when `text` is not a valid config.
#[uniffi::export]
pub fn catalog_references(
    text: &str,
    file_name: &str,
    catalog: &str,
) -> Result<Vec<SelectionEntry>, EngineError> {
    guard(|| {
        let config = parse_project_config(text, file_name)?;

        Ok(core::catalog_references(&config, catalog)
            .iter()
            .map(SelectionEntry::from)
            .collect())
    })
}

/// Checks `name` as a catalog name for the config `text`: not empty, no `/`, not already taken.
/// `renaming` is the catalog being renamed, whose own name does not count as taken.
///
/// # Errors
///
/// [`EngineError::Config`] naming the rule `name` breaks, or when `text` is not a valid config.
#[uniffi::export]
// UniFFI hands over lists and optional strings by value; it has no borrowed form for them.
#[allow(clippy::needless_pass_by_value)]
pub fn validate_catalog_name(
    text: &str,
    file_name: &str,
    name: &str,
    renaming: Option<String>,
) -> Result<(), EngineError> {
    guard(|| {
        let config = parse_project_config(text, file_name)?;

        core::validate_catalog_name(&config, name, renaming.as_deref())?;
        Ok(())
    })
}

/// A catalog name to suggest for `source`: the repository's or folder's basename, without `.git`.
/// Not checked against the names a config already uses.
#[uniffi::export]
pub fn propose_catalog_name(source: &str) -> String {
    core::propose_catalog_name(source)
}

impl ConfigEdit {
    fn into_core(self) -> Result<core::ConfigEdit, AmbitError> {
        Ok(match self {
            Self::SetHarnesses { harnesses } => core::ConfigEdit::SetHarnesses { harnesses },
            Self::AddCatalog {
                name,
                source,
                git_ref,
            } => core::ConfigEdit::AddCatalog {
                name,
                source,
                r#ref: git_ref,
            },
            Self::SetCatalogSource {
                name,
                source,
                git_ref,
            } => core::ConfigEdit::SetCatalogSource {
                name,
                source,
                r#ref: git_ref,
            },
            Self::RenameCatalog { from, to } => core::ConfigEdit::RenameCatalog { from, to },
            Self::RemoveCatalog { name } => core::ConfigEdit::RemoveCatalog { name },
            Self::AddEntry { entry } => core::ConfigEdit::AddEntry {
                entry: entry.parse()?,
            },
            Self::RemoveEntry { entry } => core::ConfigEdit::RemoveEntry {
                entry: entry.parse()?,
            },
            Self::ReplaceEntry { old, new } => core::ConfigEdit::ReplaceEntry {
                old: old.parse()?,
                new: new.parse()?,
            },
        })
    }
}

impl EntryAddress {
    /// The selection, read with the grammar a project config's `requires` uses.
    fn parse(&self) -> Result<PatternEntry, AmbitError> {
        parse_address(self.kind.into(), &self.address, Addressing::Qualified)
    }
}

impl From<core::ConfigChanges> for ConfigChanges {
    fn from(changes: core::ConfigChanges) -> Self {
        let entries = |entries: &[PatternEntry]| entries.iter().map(SelectionEntry::from).collect();
        let catalogs = |catalogs: &[CatalogRef]| catalogs.iter().map(CatalogEntry::from).collect();

        Self {
            is_empty: changes.is_empty(),
            harnesses_added: changes.harnesses_added,
            harnesses_removed: changes.harnesses_removed,
            catalogs_added: catalogs(&changes.catalogs_added),
            catalogs_removed: catalogs(&changes.catalogs_removed),
            catalogs_changed: changes
                .catalogs_changed
                .iter()
                .map(|(before, after)| CatalogChange {
                    before: before.into(),
                    after: after.into(),
                })
                .collect(),
            catalogs_renamed: changes
                .catalogs_renamed
                .into_iter()
                .map(|(from, to)| CatalogRename { from, to })
                .collect(),
            entries_added: entries(&changes.entries_added),
            entries_removed: entries(&changes.entries_removed),
        }
    }
}

#[cfg(test)]
mod tests;
