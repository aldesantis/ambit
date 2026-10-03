//! Claude plugin metadata a pack can carry for `ambit export`.

use indexmap::IndexMap;

use crate::errors::Result;
use crate::model::yaml::YamlMapping;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PluginMetadata {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub author: Option<IndexMap<String, String>>,
    pub homepage: Option<String>,
    pub repository: Option<String>,
    pub license: Option<String>,
    pub keywords: Option<Vec<String>>,
    /// External plugin names; local dependencies come from pack requirements.
    pub dependencies: Option<Vec<String>>,
    /// Output directory basename. Defaults to the plugin name.
    pub directory: Option<String>,
    /// Catalog-relative directory copied into the plugin commands directory.
    pub commands: Option<String>,
}

/// Parses export metadata without changing a pack's installation requirements.
///
/// # Errors
///
/// Exit 2 for a malformed `plugin` block.
pub fn parse_plugin_metadata(mapping: &YamlMapping) -> Result<PluginMetadata> {
    let _ = mapping;
    todo!("port model/plugin.ts:parsePluginMetadata")
}
