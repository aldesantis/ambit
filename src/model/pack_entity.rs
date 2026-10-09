use crate::errors::Result;
use crate::model::pattern::{Addressing, PatternEntry, REQUIRES_KEY, parse_entries};
use crate::model::plugin::{PluginMetadata, parse_plugin_metadata};
use crate::model::yaml::YamlMapping;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackEntity {
    pub name: String,
    pub plugin: Option<PluginMetadata>,
    pub description: Option<String>,
    pub requires: Vec<PatternEntry>,
}

const ENTITY_KEYS: &[&str] = &["description", "name", "plugin", REQUIRES_KEY];

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
        requires: parse_entries(mapping, Addressing::Unqualified)?,
    })
}
