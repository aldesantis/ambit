//! Pack documents: `packs/<name>.yml` in a catalog.

use crate::errors::Result;
use crate::model::pattern::{Addressing, PatternEntry, REQUIRES_KEY, parse_entries};
use crate::model::plugin::{PluginMetadata, parse_plugin_metadata};
use crate::model::yaml::YamlMapping;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackEntity {
    pub name: String,
    pub plugin: Option<PluginMetadata>,
    /// Shown by `ambit search`.
    pub description: Option<String>,
    /// What asking for this pack gets you: a `requires` list in the same entry grammar a project
    /// selects with, minus the qualifier; see [`PatternEntry`]. In the order the author wrote them.
    ///
    /// May name other packs, which lets a small pack compose into a large one; the resolution
    /// closure follows these to a fixpoint. Entries are unqualified and so confined to this
    /// catalog: the qualifier is a consumer-config alias a catalog author cannot write, so a
    /// catalog can only require what it ships.
    pub requires: Vec<PatternEntry>,
}

/// The keys a pack document may hold.
///
/// No `expects`, unlike the other three kinds. An expectation is read by something that runs: a
/// skill's instructions, a server's credentials, a hook's command. A pack runs nothing itself; each
/// item it names carries its own `expects` into the union instead.
const ENTITY_KEYS: &[&str] = &["description", "name", "plugin", REQUIRES_KEY];

/// Parses a pack document.
///
/// # Errors
///
/// Exit 2 for a missing or malformed `name`, an unknown key, or a `requires` entry the grammar
/// refuses.
pub fn parse_pack_entity(mapping: &YamlMapping) -> Result<PackEntity> {
    mapping.reject_unknown_keys(ENTITY_KEYS)?;

    let name = mapping.require_string("name")?;
    let description = mapping.optional_string("description")?;
    let plugin = mapping
        .optional_mapping("plugin")?
        .map(|plugin| parse_plugin_metadata(&plugin))
        .transpose()?;

    Ok(PackEntity {
        name,
        plugin,
        description,
        // Unqualified: a catalog author cannot write a consumer's alias, so the pattern stands
        // alone and the entry resolves within this catalog.
        requires: parse_entries(mapping, Addressing::Unqualified)?,
    })
}
