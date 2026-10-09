use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::errors::Result;
use crate::export::files::{PackageFiles, io_failed, mode_of};
use crate::util::cmp::js_cmp;
use crate::util::fs::{canonicalize, read_dir_names};
use crate::util::path::{join, relative};
use crate::util::string_enum;

string_enum! {
    pub enum LinkType {
        Dir => "dir",
        File => "file",
    }
}

// Never equal to any mode an export writes.
pub const UNSUPPORTED_MODE: u32 = u32::MAX;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportEntry {
    pub data: Option<Vec<u8>>,
    pub mode: u32,
    pub link: Option<String>,
    pub link_type: Option<LinkType>,
}

pub type ExportTree = IndexMap<String, ExportEntry>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreePackage {
    pub directory: String,
    pub files: PackageFiles,
}

static SKILL_DIRECTORY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^skills/[^/]+$").expect("a valid pattern"));

pub fn canonical_path(target: &Path) -> Result<PathBuf> {
    match canonicalize(target) {
        Ok(resolved) => Ok(resolved),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match (target.parent(), target.file_name()) {
                (Some(parent), Some(name)) => {
                    let parent = if parent.as_os_str().is_empty() {
                        Path::new(".")
                    } else {
                        parent
                    };

                    Ok(canonical_path(parent)?.join(name))
                }
                _ => Err(io_failed(&error, target)),
            }
        }
        Err(error) => Err(io_failed(&error, target)),
    }
}

fn posix_dirname(name: &str) -> &str {
    match name.trim_end_matches('/').rfind('/') {
        Some(0) => "/",
        Some(index) => &name[..index],
        None => ".",
    }
}

pub fn package_tree(packages: &[TreePackage], output: &Path, link: bool) -> ExportTree {
    fn add(tree: &mut ExportTree, name: &str, entry: ExportEntry) {
        let parent = posix_dirname(name);

        if parent != "." && !tree.contains_key(parent) {
            add(
                tree,
                parent,
                ExportEntry {
                    data: None,
                    mode: 0o755,
                    link: None,
                    link_type: None,
                },
            );
        }

        tree.insert(name.to_owned(), entry);
    }

    let mut tree = ExportTree::new();

    for item in packages {
        let mut linked_directories: Vec<String> = Vec::new();

        for (file_path, file) in &item.files {
            if linked_directories
                .iter()
                .any(|directory| file_path.starts_with(&format!("{directory}/")))
            {
                continue;
            }

            let name = format!("{}/{file_path}", item.directory);
            let linkable = (file.data.is_none() && SKILL_DIRECTORY.is_match(file_path))
                || (file.data.is_some() && file_path.starts_with("hooks/"));

            match &file.source {
                Some(source) if link && linkable => {
                    let at = join(output, &name);
                    let from = at.parent().unwrap_or(output);

                    add(
                        &mut tree,
                        &name,
                        ExportEntry {
                            data: None,
                            mode: 0o777,
                            link: Some(relative(from, source)),
                            link_type: Some(if file.data.is_none() {
                                LinkType::Dir
                            } else {
                                LinkType::File
                            }),
                        },
                    );

                    if file.data.is_none() {
                        linked_directories.push(file_path.clone());
                    }
                }
                _ => add(
                    &mut tree,
                    &name,
                    ExportEntry {
                        data: file.data.clone(),
                        mode: file.mode,
                        link: None,
                        link_type: None,
                    },
                ),
            }
        }
    }

    tree
}

pub fn read_tree(root: &Path) -> Result<ExportTree> {
    fn visit(root: &Path, relative: &str, tree: &mut ExportTree) -> Result<()> {
        let target = if relative.is_empty() {
            root.to_path_buf()
        } else {
            join(root, relative)
        };
        let info =
            std::fs::symlink_metadata(&target).map_err(|error| io_failed(&error, &target))?;

        if info.file_type().is_symlink() {
            let link = std::fs::read_link(&target).map_err(|error| io_failed(&error, &target))?;

            tree.insert(
                relative.to_owned(),
                ExportEntry {
                    data: None,
                    mode: 0o777,
                    link: Some(link.to_string_lossy().into_owned()),
                    link_type: None,
                },
            );
        } else if info.is_dir() {
            if !relative.is_empty() {
                tree.insert(
                    relative.to_owned(),
                    ExportEntry {
                        data: None,
                        mode: mode_of(&info),
                        link: None,
                        link_type: None,
                    },
                );
            }

            let mut names = read_dir_names(&target).map_err(|error| io_failed(&error, &target))?;
            names.sort_by(|a, b| js_cmp(a, b));

            for name in names {
                let child = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };

                visit(root, &child, tree)?;
            }
        } else if info.is_file() {
            let data = std::fs::read(&target).map_err(|error| io_failed(&error, &target))?;

            tree.insert(
                relative.to_owned(),
                ExportEntry {
                    data: Some(data),
                    mode: mode_of(&info),
                    link: None,
                    link_type: None,
                },
            );
        } else {
            tree.insert(
                relative.to_owned(),
                ExportEntry {
                    data: None,
                    mode: UNSUPPORTED_MODE,
                    link: None,
                    link_type: None,
                },
            );
        }

        Ok(())
    }

    let mut tree = ExportTree::new();
    visit(root, "", &mut tree)?;

    Ok(tree)
}

pub fn same_entry(name: &str, expected: &ExportEntry, actual: &ExportEntry) -> bool {
    if expected.link != actual.link {
        return false;
    }

    if expected.link.is_some() {
        return true;
    }

    let (Some(expected_data), Some(actual_data)) = (&expected.data, &actual.data) else {
        return expected.data == actual.data && actual.mode != UNSUPPORTED_MODE;
    };

    if (expected.mode & 0o111) != (actual.mode & 0o111) {
        return false;
    }

    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let is_json = name.ends_with(".json");

    if is_json {
        let parse = |data: &[u8]| crate::util::json::parse(&String::from_utf8_lossy(data)).ok();

        return match (parse(expected_data), parse(actual_data)) {
            (Some(left), Some(right)) => crate::util::json::structurally_equal(&left, &right),
            _ => false,
        };
    }

    expected_data == actual_data
}
