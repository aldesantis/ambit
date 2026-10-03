//! Rendering one plugin bundle as a Claude plugin package.

use std::path::Path;

use crate::errors::Result;
use crate::export::files::PackageFiles;
use crate::export::resolve::PluginBundle;

/// Renders a complete Claude package without reading credential values or executing assets.
///
/// # Errors
///
/// Exit 2 for an asset that cannot be packaged.
pub fn render_claude_plugin(plugin: &PluginBundle, catalog_root: &Path) -> Result<PackageFiles> {
    let _ = (plugin, catalog_root);
    todo!("port export/claude.ts:renderClaudePlugin")
}

/// Checks explicit Claude skill invocations against the plugin dependency graph.
///
/// # Errors
///
/// Exit 2 for a skill invocation no dependency provides.
pub fn validate_skill_references(
    plugins: &[PluginBundle],
    rendered: &[PackageFiles],
) -> Result<()> {
    let _ = (plugins, rendered);
    todo!("port export/claude.ts:validateSkillReferences")
}
