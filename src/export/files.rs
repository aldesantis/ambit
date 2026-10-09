use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::{AmbitError, Result, config_error};
use crate::util::cmp::js_cmp;
use crate::util::fs::{canonicalize, io_message, read_dir_names};
use crate::util::path::relative;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageFile {
    pub data: Option<Vec<u8>>,
    pub mode: u32,
    pub source: Option<PathBuf>,
}

pub type PackageFiles = IndexMap<String, PackageFile>;

pub(crate) fn io_failed(error: &std::io::Error, path: &Path) -> AmbitError {
    config_error(
        "cannot export Claude plugins",
        [
            io_message(error, path),
            "check the source files and output directory permissions".to_owned(),
        ],
    )
}

pub(crate) fn is_within(root: &Path, target: &Path) -> bool {
    let relative = relative(root, target);
    let parent = format!("..{}", std::path::MAIN_SEPARATOR);

    relative.is_empty()
        || (!relative.starts_with(&parent)
            && relative != ".."
            && !Path::new(&relative).is_absolute())
}

pub(crate) fn mode_of(metadata: &std::fs::Metadata) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        metadata.permissions().mode() & 0o777
    }

    #[cfg(not(unix))]
    {
        if metadata.is_dir() {
            0o777
        } else if metadata.permissions().readonly() {
            0o444
        } else {
            0o666
        }
    }
}

pub fn add_file(files: &mut PackageFiles, target: &str, data: Vec<u8>, mode: u32) -> Result<()> {
    let nested = format!("{target}/");

    if files.contains_key(target) || files.keys().any(|file| file.starts_with(&nested)) {
        return Err(config_error(
            format!("export path collision at {target}"),
            ["rename the conflicting skill or hook asset"],
        ));
    }

    files.insert(
        target.to_owned(),
        PackageFile {
            data: Some(data),
            mode,
            source: None,
        },
    );

    Ok(())
}

pub fn collect_files(
    files: &mut PackageFiles,
    source: &Path,
    destination: &str,
    catalog_root: &Path,
    exclude: &[String],
) -> Result<()> {
    let root = canonicalize(catalog_root).map_err(|error| io_failed(&error, catalog_root))?;
    let metadata = std::fs::metadata(source).map_err(|error| io_failed(&error, source))?;

    if !metadata.is_dir() {
        return Err(config_error(
            format!("{}: expected an asset directory", source.display()),
            ["point this component at a directory inside the catalog"],
        ));
    }

    let walker = Walker {
        root: &root,
        exclude,
    };

    walker.walk(files, source, destination, &[], true)
}

struct Walker<'a> {
    root: &'a Path,
    exclude: &'a [String],
}

impl Walker<'_> {
    fn walk(
        &self,
        files: &mut PackageFiles,
        file: &Path,
        target: &str,
        ancestors: &[PathBuf],
        top: bool,
    ) -> Result<()> {
        let actual = canonicalize(file).map_err(|error| io_failed(&error, file))?;

        if !is_within(self.root, &actual) {
            return Err(config_error(
                format!("{}: asset escapes its catalog", file.display()),
                ["move the asset into the catalog before exporting"],
            ));
        }

        if ancestors.contains(&actual) {
            return Err(config_error(
                format!("{}: cyclic asset symlink", file.display()),
                ["remove the cycle before exporting"],
            ));
        }

        let info = std::fs::metadata(&actual).map_err(|error| io_failed(&error, &actual))?;

        if info.is_dir() {
            if files
                .get(target)
                .is_some_and(|existing| existing.data.is_some())
            {
                return Err(config_error(
                    format!("export path collision at {target}"),
                    ["rename the conflicting asset"],
                ));
            }

            files.insert(
                target.to_owned(),
                PackageFile {
                    data: None,
                    mode: mode_of(&info),
                    source: Some(actual.clone()),
                },
            );

            let mut next = ancestors.to_vec();
            next.push(actual.clone());

            let mut names = read_dir_names(&actual).map_err(|error| io_failed(&error, &actual))?;
            names.sort_by(|a, b| js_cmp(a, b));

            for name in names {
                if top && self.exclude.contains(&name) {
                    continue;
                }

                self.walk(
                    files,
                    &actual.join(&name),
                    &format!("{target}/{name}"),
                    &next,
                    false,
                )?;
            }
        } else if info.is_file() {
            let data = std::fs::read(&actual).map_err(|error| io_failed(&error, &actual))?;

            add_file(files, target, data, mode_of(&info))?;

            if let Some(added) = files.get_mut(target) {
                added.source = Some(actual);
            }
        } else {
            return Err(config_error(
                format!("{}: unsupported asset type", file.display()),
                ["package only regular files and directories"],
            ));
        }

        Ok(())
    }
}
