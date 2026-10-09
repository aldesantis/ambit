use indexmap::IndexSet;

use crate::errors::{Result, config_error, resolution_error};
use crate::model::catalog::{MergedCatalog, MergedPack};
use crate::model::config::ProjectConfig;
use crate::model::pattern::{PatternEntry, PatternItem, matches};
use crate::model::plugin::PluginMetadata;
use crate::model::requirement::ItemKind;
use crate::resolution::resolve::{
    Bundle, Requirer, RequirerKind, assert_entries_match, close_over_requires, required_items,
    resolve_bundle,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginBundle {
    pub pack: MergedPack,
    pub metadata: PluginMetadata,
    pub directory: String,
    pub dependencies: Vec<String>,
    pub bundle: Bundle,
}

fn pack_requirer(pack: &MergedPack) -> Requirer {
    Requirer {
        kind: RequirerKind::Pack,
        catalog: pack.catalog.clone(),
        name: pack.name.clone(),
        requires: pack.requires.clone(),
        file: pack.file.clone(),
    }
}

fn same_pack(a: &MergedPack, b: &MergedPack) -> bool {
    a.catalog == b.catalog && a.name == b.name
}

pub fn resolve_plugins(
    config: &ProjectConfig,
    merged: &MergedCatalog,
) -> Result<Vec<PluginBundle>> {
    if config.requires.is_empty()
        || config
            .requires
            .iter()
            .any(|entry| entry.kind != ItemKind::Pack)
    {
        return Err(config_error(
            format!(
                "{}: export requires a selection of packs",
                config.origin.file
            ),
            ["select exportable packs with `requires: [{ pack: catalog/name }]`"],
        ));
    }

    assert_entries_match(config, merged)?;

    let roots: Vec<&MergedPack> = merged
        .packs
        .iter()
        .filter(|pack| {
            config.requires.iter().any(|entry| {
                matches(
                    entry,
                    PatternItem {
                        kind: ItemKind::Pack,
                        catalog: &pack.catalog,
                        name: &pack.name,
                    },
                )
            })
        })
        .collect();

    for pack in &roots {
        if pack.plugin.is_none() {
            return Err(config_error(
                format!(
                    "{}: pack \"{}\" has no plugin metadata",
                    pack.file, pack.name
                ),
                ["add a `plugin` mapping with at least a `name`"],
            ));
        }
    }

    // Validate the complete graph before cutting boundary edges, so cycles cannot disappear.
    let requirers: Vec<Requirer> = roots.iter().map(|pack| pack_requirer(pack)).collect();
    let selection = close_over_requires(&requirers, &[], &[], merged)?;
    let mut names: IndexSet<String> = IndexSet::new();
    let mut directories: IndexSet<String> = IndexSet::new();
    let mut plugins = Vec::new();

    for pack in selection.packs.iter().filter(|pack| pack.plugin.is_some()) {
        let metadata = pack
            .plugin
            .clone()
            .expect("filtered to packs with metadata");
        let directory = metadata
            .directory
            .clone()
            .unwrap_or_else(|| metadata.name.clone());

        if names.contains(&metadata.name) || directories.contains(&directory) {
            return Err(resolution_error(
                format!(
                    "{}: duplicate plugin name or output directory \"{directory}\"",
                    pack.file
                ),
                ["give each exported pack a distinct plugin name and directory"],
            ));
        }

        names.insert(metadata.name.clone());
        directories.insert(directory.clone());

        let bounded = MergedCatalog {
            packs: merged
                .packs
                .iter()
                .map(|other| {
                    if !same_pack(other, pack) && other.plugin.is_some() {
                        MergedPack {
                            requires: Vec::new(),
                            ..other.clone()
                        }
                    } else {
                        other.clone()
                    }
                })
                .collect(),
            ..merged.clone()
        };
        let own = ProjectConfig {
            requires: vec![PatternEntry {
                kind: ItemKind::Pack,
                pattern: pack.name.clone(),
                catalog: Some(pack.catalog.clone()),
            }],
            ..config.clone()
        };
        let bundle = resolve_bundle(&own, &bounded)?;
        let mut dependencies: IndexSet<String> = metadata
            .dependencies
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let mut visited: IndexSet<String> = IndexSet::new();

        follow(
            &pack_requirer(pack),
            merged,
            &mut dependencies,
            &mut visited,
        );

        plugins.push(PluginBundle {
            pack: pack.clone(),
            metadata,
            directory,
            dependencies: dependencies.into_iter().collect(),
            bundle,
        });
    }

    Ok(plugins)
}

fn follow(
    node: &Requirer,
    merged: &MergedCatalog,
    dependencies: &mut IndexSet<String>,
    visited: &mut IndexSet<String>,
) {
    let key = format!("{}:{}/{}", node.kind, node.catalog, node.name);

    if !visited.insert(key) {
        return;
    }

    for entry in &node.requires {
        let found = required_items(entry, node, merged);

        for child in &found.packs {
            match &child.plugin {
                Some(plugin) => {
                    dependencies.insert(plugin.name.clone());
                }
                None => follow(&pack_requirer(child), merged, dependencies, visited),
            }
        }

        for child in &found.skills {
            let requirer = Requirer {
                kind: RequirerKind::Skill,
                catalog: child.catalog.clone(),
                name: child.name.clone(),
                requires: child.requires.clone(),
                file: format!("{}/SKILL.md", child.path),
            };

            follow(&requirer, merged, dependencies, visited);
        }
    }
}
