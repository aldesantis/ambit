use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::{AmbitError, Result, config_error};
use crate::model::config::ProjectConfig;
use crate::model::git::is_commit_sha;
use crate::model::sources::{Source, SourceRequest, parse_source};
use crate::model::yaml::{YamlMapping, parse_yaml_mapping};
use crate::util::fs::{io_message, read_text_opt};
use crate::util::path::join;

pub const LOCK_FILENAME: &str = "ambit.lock";

pub const LOCK_VERSION: i64 = 1;

pub fn lock_file_path(project_dir: &Path) -> PathBuf {
    join(project_dir, LOCK_FILENAME)
}

pub fn read_lock_text(project_dir: &Path) -> Result<Option<String>> {
    let file = lock_file_path(project_dir);

    read_text_opt(&file).map_err(|error| {
        config_error(
            format!("cannot read {LOCK_FILENAME}"),
            [
                io_message(&error, &file),
                format!(
                    "make {} readable, or delete it and run `ambit install` again",
                    file.display()
                ),
            ],
        )
    })
}

struct RecordedCatalog {
    source: String,
    r#ref: Option<String>,
    commit: Option<String>,
}

fn unsupported_version(found: i64) -> AmbitError {
    config_error(
        format!("{LOCK_FILENAME} is version {found}, which this build cannot read"),
        [
            format!(
                "ambit resolves against the commits a lock records, and only version {LOCK_VERSION} is a shape it knows"
            ),
            format!("upgrade ambit, or delete {LOCK_FILENAME} and run `ambit install` again"),
        ],
    )
}

fn read_lock_root(project_dir: &Path) -> Result<Option<YamlMapping>> {
    let Some(text) = read_lock_text(project_dir)? else {
        return Ok(None);
    };

    let root = parse_yaml_mapping(&text, LOCK_FILENAME)?;
    let version = root.require_integer("version")?;

    if version != LOCK_VERSION {
        return Err(unsupported_version(version));
    }

    Ok(Some(root))
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockedItem {
    pub catalog: Option<String>,
    pub path: Option<String>,
    pub commit: Option<String>,
    pub digest: Option<String>,
    pub exec: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LockedItems {
    pub catalog_commits: IndexMap<String, String>,
    pub skills: IndexMap<String, LockedItem>,
    pub mcps: IndexMap<String, LockedItem>,
    pub hooks: IndexMap<String, LockedItem>,
}

fn locked_section(root: &YamlMapping, key: &str) -> Result<IndexMap<String, LockedItem>> {
    let Some(section) = root.optional_mapping(key)? else {
        return Ok(IndexMap::new());
    };

    let mut items = IndexMap::new();

    for name in section.keys() {
        let entry = section.require_mapping(&name)?;

        items.insert(
            name,
            LockedItem {
                catalog: entry.optional_string("catalog")?,
                path: entry.optional_string("path")?,
                commit: entry.optional_string("commit")?,
                digest: entry.optional_string("digest")?,
                exec: entry.optional_string("exec")?,
            },
        );
    }

    Ok(items)
}

fn catalog_commits(root: &YamlMapping) -> Result<IndexMap<String, String>> {
    let Some(section) = root.optional_mapping("catalogs")? else {
        return Ok(IndexMap::new());
    };

    let mut commits = IndexMap::new();

    for name in section.keys() {
        if let Some(commit) = section.require_mapping(&name)?.optional_string("commit")? {
            commits.insert(name, commit);
        }
    }

    Ok(commits)
}

pub fn read_locked_items(project_dir: &Path) -> Result<Option<LockedItems>> {
    let Some(root) = read_lock_root(project_dir)? else {
        return Ok(None);
    };

    Ok(Some(LockedItems {
        catalog_commits: catalog_commits(&root)?,
        skills: locked_section(&root, "skills")?,
        mcps: locked_section(&root, "mcps")?,
        hooks: locked_section(&root, "hooks")?,
    }))
}

/// Unknown keys are deliberately not rejected, so a lock written by a later ambit still reads.
fn read_recorded_catalogs(project_dir: &Path) -> Result<Option<IndexMap<String, RecordedCatalog>>> {
    let Some(root) = read_lock_root(project_dir)? else {
        return Ok(None);
    };

    let Some(catalogs) = root.optional_mapping("catalogs")? else {
        return Ok(Some(IndexMap::new()));
    };

    let mut recorded = IndexMap::new();

    for name in catalogs.keys() {
        let entry = catalogs.require_mapping(&name)?;
        let commit = entry.optional_string("commit")?;
        let r#ref = entry.optional_string("ref")?;

        if let Some(commit) = &commit
            && !is_commit_sha(commit)
        {
            return Err(entry.key_error(
                "commit",
                &format!("catalog \"{name}\" is pinned to something that is not a commit"),
                vec![
                    format!("\"{commit}\" is not a full commit SHA"),
                    format!(
                        "delete {LOCK_FILENAME} and run `ambit install` again to write a correct one"
                    ),
                ],
            ));
        }

        let source = entry.require_string("source")?;

        recorded.insert(
            name,
            RecordedCatalog {
                source,
                r#ref,
                commit,
            },
        );
    }

    Ok(Some(recorded))
}

fn git_identity(source: &str, r#ref: Option<&str>) -> Option<String> {
    let request = SourceRequest {
        source: source.to_owned(),
        r#ref: r#ref.map(str::to_owned),
        ..SourceRequest::default()
    };

    match parse_source(&request) {
        Ok(Source::Git { url, r#ref }) => Some(format!("{url} {}", r#ref.unwrap_or_default())),
        Ok(Source::Path { .. }) | Err(_) => None,
    }
}

pub fn read_catalog_pins(
    project_dir: &Path,
    config: &ProjectConfig,
) -> Result<IndexMap<String, String>> {
    let Some(recorded) = read_recorded_catalogs(project_dir)? else {
        return Ok(IndexMap::new());
    };

    let mut pins = IndexMap::new();

    for entry in &config.catalogs {
        let Some(locked) = recorded.get(&entry.name) else {
            continue;
        };

        let Some(commit) = &locked.commit else {
            continue;
        };

        let Some(configured) = git_identity(&entry.source, entry.r#ref.as_deref()) else {
            continue;
        };

        if git_identity(&locked.source, locked.r#ref.as_deref()).as_ref() != Some(&configured) {
            continue;
        }

        pins.insert(entry.name.clone(), commit.clone());
    }

    Ok(pins)
}
