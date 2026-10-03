//! `ambit export`: selected packs as Claude plugins.

pub mod claude;
pub mod files;
pub mod resolve;
pub mod tree;

use crate::errors::Result;
use crate::model::sources::SourceContext;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    pub output: String,
    pub dry_run: bool,
    pub link: bool,
    pub force: bool,
    pub check: bool,
}

/// One exported plugin, as the command reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportedPlugin {
    pub name: String,
    pub directory: String,
    pub files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportResult {
    pub output: String,
    pub plugins: Vec<ExportedPlugin>,
}

/// Exports or checks selected packs, honoring existing catalog lock pins.
///
/// # Errors
///
/// Exit 2 for invalid packages or an existing output; exit 3 for resolution errors; exit 5 for
/// drift.
pub fn export_plugins(context: &SourceContext, options: &ExportOptions) -> Result<ExportResult> {
    let _ = (context, options);
    todo!("port export/export.ts:exportPlugins")
}
