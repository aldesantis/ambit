//! Pack documents: `packs/<name>.yml` in a catalog.

use crate::errors::Result;
use crate::model::pattern::PatternEntry;
use crate::model::plugin::PluginMetadata;
use crate::model::yaml::YamlMapping;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackEntity {
    pub name: String,
    pub plugin: Option<PluginMetadata>,
    /// Shown by `ambit search`.
    pub description: Option<String>,
    /// What asking for this pack gets you: a `requires` list in the same entry grammar a project
    /// selects with, minus the qualifier. In the order the author wrote them.
    ///
    /// May name other packs, which lets a small pack compose into a large one; the resolution
    /// closure follows these to a fixpoint. Entries are unqualified and so confined to this
    /// catalog.
    pub requires: Vec<PatternEntry>,
}

/// Parses a pack document.
///
/// # Errors
///
/// Exit 2 for a missing or malformed `name`, an unknown key, or a `requires` entry the grammar
/// refuses.
pub fn parse_pack_entity(mapping: &YamlMapping) -> Result<PackEntity> {
    let _ = mapping;
    todo!("port model/pack-entity.ts:parsePackEntity")
}
