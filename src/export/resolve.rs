//! Resolving each exported plugin's own bundle.

use crate::errors::Result;
use crate::model::catalog::{MergedCatalog, MergedPack};
use crate::model::config::ProjectConfig;
use crate::model::plugin::PluginMetadata;
use crate::resolution::resolve::Bundle;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginBundle {
    pub pack: MergedPack,
    pub metadata: PluginMetadata,
    pub directory: String,
    pub dependencies: Vec<String>,
    pub bundle: Bundle,
}

/// Resolves each plugin separately, stopping content expansion at other plugin packs.
///
/// # Errors
///
/// Exit 2 for invalid plugin metadata; exit 3 for a resolution error.
pub fn resolve_plugins(
    config: &ProjectConfig,
    merged: &MergedCatalog,
) -> Result<Vec<PluginBundle>> {
    let _ = (config, merged);
    todo!("port export/resolve.ts:resolvePlugins")
}
