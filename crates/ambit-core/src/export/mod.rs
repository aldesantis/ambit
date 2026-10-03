//! `ambit export`: selected packs as Claude plugins.
//!
//! The export is built in full in memory, compared or written as one tree, and swapped into place
//! by rename, so a failed export never leaves a half-written directory where the previous one was.

pub mod claude;
pub mod files;
pub mod resolve;
pub mod tree;

use std::path::{Path, PathBuf};

use indexmap::IndexSet;

use crate::errors::{Result, config_error, drift_error};
use crate::export::claude::{render_claude_plugin, validate_skill_references};
use crate::export::files::{PackageFiles, io_failed, is_within};
use crate::export::resolve::resolve_plugins;
use crate::export::tree::{
    ExportEntry, ExportTree, LinkType, TreePackage, canonical_path, package_tree, read_tree,
    same_entry,
};
use crate::model::catalog::{CatalogLoadOptions, load_catalogs, merge_catalogs};
use crate::model::config::load_project_config;
use crate::model::sources::SourceContext;
use crate::util::cmp::js_cmp;
use crate::util::fs::{mkdir_p, rm_rf, symlink_dir, symlink_file};
use crate::util::path::{join, resolve};

/// The prefix of the staging and backup directories made beside the output.
const STAGING_PREFIX: &str = ".ambit-export-";

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

/// A fresh directory beside `output`, kept until the caller removes or renames it.
fn sibling_tempdir(output: &Path) -> Result<PathBuf> {
    let parent = output.parent().unwrap_or(output);

    tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(parent)
        .map(tempfile::TempDir::keep)
        .map_err(|error| io_failed(&error, "mkdtemp", parent))
}

/// Sets a written file's permission bits. Windows has none beyond read-only, so nothing is set
/// there.
fn set_mode(target: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode))
    }

    #[cfg(not(unix))]
    {
        let _ = (target, mode);
        Ok(())
    }
}

/// Writes `tree` under `staging`, reusing the bytes of unchanged files from `current`.
fn write_tree(staging: &Path, tree: &ExportTree, current: &ExportTree) -> Result<()> {
    for (relative, file) in tree {
        let target = join(staging, relative);

        if let Some(link) = &file.link {
            let created = if file.link_type == Some(LinkType::Dir) {
                symlink_dir(Path::new(link), &target)
            } else {
                symlink_file(Path::new(link), &target)
            };

            created.map_err(|error| io_failed(&error, "symlink", &target))?;
        } else if let Some(data) = &file.data {
            // Retain JSON formatting for unchanged values to avoid unrelated marketplace diffs.
            let data = match current.get(relative) {
                Some(
                    previous @ ExportEntry {
                        data: Some(bytes), ..
                    },
                ) if same_entry(relative, file, previous) => bytes,
                _ => data,
            };

            std::fs::write(&target, data).map_err(|error| io_failed(&error, "open", &target))?;
            set_mode(&target, file.mode).map_err(|error| io_failed(&error, "chmod", &target))?;
        } else {
            mkdir_p(&target).map_err(|error| io_failed(&error, "mkdir", &target))?;
        }
    }

    Ok(())
}

/// Moves `staging` to `output`, replacing an existing export through a backup that is restored
/// if the second rename fails.
fn swap_into_place(staging: &Path, output: &Path, existing: bool) -> Result<()> {
    if existing {
        let backup = sibling_tempdir(output)?;
        let previous = backup.join("previous");
        let replaced = std::fs::rename(output, &previous).and_then(|()| {
            std::fs::rename(staging, output).or_else(|error| {
                std::fs::rename(&previous, output)?;
                Err(error)
            })
        });

        return match replaced {
            Ok(()) => {
                // The new export is in place; a leftover backup is only clutter.
                let _ = rm_rf(&backup);
                Ok(())
            }
            // Keep the backup available if restoring the previous export also failed.
            Err(error) => Err(config_error(
                format!("cannot replace export at {}", output.display()),
                [
                    format!(
                        "Error: {}",
                        crate::util::fs::io_message(&error, "rename", output)
                    ),
                    format!("previous export backup: {}", backup.display()),
                ],
            )),
        };
    }

    // Reserve exclusively so concurrent exports cannot replace another writer's output.
    std::fs::create_dir(output).map_err(|error| io_failed(&error, "mkdir", output))?;

    if let Err(error) = std::fs::rename(staging, output) {
        let _ = rm_rf(output);
        return Err(io_failed(&error, "rename", staging));
    }

    Ok(())
}

/// Exports or checks selected packs, honoring existing catalog lock pins.
///
/// # Errors
///
/// Exit 2 for invalid packages or an existing output; exit 3 for resolution errors; exit 5 for
/// drift.
///
/// # Panics
///
/// Never: every merged pack comes from one of the loaded catalogs.
pub fn export_plugins(context: &SourceContext, options: &ExportOptions) -> Result<ExportResult> {
    let output = resolve(&context.project_dir, &options.output);

    if options.check && (options.force || options.dry_run) {
        return Err(config_error(
            "--check cannot be combined with --force or --dry-run",
            Vec::<String>::new(),
        ));
    }

    let existing = match std::fs::symlink_metadata(&output) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(io_failed(&error, "lstat", &output)),
    };

    if existing.is_some() && !options.force && !options.check {
        return Err(config_error(
            format!("export output already exists: {}", output.display()),
            ["use --force to regenerate it or --check to check for drift"],
        ));
    }

    if let Some(metadata) = &existing
        && (!metadata.is_dir() || metadata.file_type().is_symlink())
    {
        return Err(config_error(
            format!(
                "export output must be a regular directory: {}",
                output.display()
            ),
            Vec::<String>::new(),
        ));
    }

    let config = load_project_config(&context.project_dir)?;

    if options.link
        && config
            .catalogs
            .iter()
            .any(|catalog| !catalog.source.starts_with("path:"))
    {
        return Err(config_error(
            "linked exports require local path catalogs",
            ["use local catalogs or omit --link for a standalone export"],
        ));
    }

    let catalogs = load_catalogs(&config, context, &mut CatalogLoadOptions::default())?;
    let plugins = resolve_plugins(&config, &merge_catalogs(&catalogs))?;
    let mut rendered: Vec<PackageFiles> = Vec::with_capacity(plugins.len());

    for plugin in &plugins {
        let catalog = catalogs
            .iter()
            .find(|catalog| catalog.name == plugin.pack.catalog)
            .expect("every merged pack comes from a loaded catalog");

        rendered.push(render_claude_plugin(plugin, &catalog.root)?);
    }

    validate_skill_references(&plugins, &rendered)?;

    let result = ExportResult {
        output: output.to_string_lossy().into_owned(),
        plugins: plugins
            .iter()
            .zip(&rendered)
            .map(|(plugin, files)| ExportedPlugin {
                name: plugin.metadata.name.clone(),
                directory: plugin.directory.clone(),
                files: files.values().filter(|file| file.data.is_some()).count(),
            })
            .collect(),
    };
    let final_output = canonical_path(&output)?;

    if options.force {
        let roots = std::iter::once(context.project_dir.clone())
            .chain(catalogs.iter().map(|catalog| catalog.root.clone()))
            .map(|root| (root, false));
        let assets = plugins.iter().flat_map(|plugin| {
            let skills = plugin
                .bundle
                .skills
                .iter()
                .map(|skill| join(&skill.catalog_root, &skill.path));
            let hooks = plugin
                .bundle
                .hooks
                .iter()
                .map(|hook| join(&hook.catalog_root, &hook.path));

            skills.chain(hooks).map(|asset| (asset, true))
        });

        for (source, is_asset) in roots.chain(assets) {
            let actual = canonical_path(&source)?;

            if is_within(&final_output, &actual) || (is_asset && is_within(&actual, &final_output))
            {
                return Err(config_error(
                    format!(
                        "export output contains source files or overlaps source assets: {}",
                        output.display()
                    ),
                    ["choose a directory outside the catalog's skills and hooks"],
                ));
            }
        }
    }

    let packages: Vec<TreePackage> = plugins
        .iter()
        .zip(rendered)
        .map(|(plugin, files)| TreePackage {
            directory: plugin.directory.clone(),
            files,
        })
        .collect();
    let tree = package_tree(&packages, &final_output, options.link);
    let current = if existing.is_some() {
        read_tree(&output)?
    } else {
        ExportTree::new()
    };

    if options.check {
        let mut names: Vec<&String> = tree
            .keys()
            .chain(current.keys())
            .collect::<IndexSet<_>>()
            .into_iter()
            .collect();
        names.sort_by(|a, b| js_cmp(a, b));

        let differences: Vec<String> = names
            .into_iter()
            .filter(|name| match (tree.get(*name), current.get(*name)) {
                (Some(expected), Some(actual)) => !same_entry(name, expected, actual),
                _ => true,
            })
            .cloned()
            .collect();

        if existing.is_none() || !differences.is_empty() {
            return Err(drift_error(
                format!("export differs from {}", output.display()),
                differences
                    .into_iter()
                    .chain(["run export with --force to regenerate it".to_owned()]),
            ));
        }

        return Ok(result);
    }

    if options.dry_run {
        return Ok(result);
    }

    if let Some(parent) = output.parent() {
        mkdir_p(parent).map_err(|error| io_failed(&error, "mkdir", parent))?;
    }

    let staging = sibling_tempdir(&output)?;
    let written = write_tree(&staging, &tree, &current)
        .and_then(|()| swap_into_place(&staging, &output, existing.is_some()));

    if written.is_err() {
        let _ = rm_rf(&staging);
    }

    written.map(|()| result)
}

#[cfg(all(test, feature = "cli"))]
mod tests;
