//! A digest of everything on disk a reviewed plan depends on, so applying it can tell whether the
//! project moved underneath the review.
//!
//! Covered: both accepted config names, `ambit.lock`, `.ambit/state.json`, the two `.gitignore`
//! files ambit keeps a block in, and the contents of every `path:` catalog the config names. A git
//! catalog is not hashed: a review records its commit and applying it installs exactly that
//! commit. Target ownership is not hashed either: it is checked again in full right before
//! anything is written.
//!
//! A file is hashed as one of three states, absent, not a regular file, or its bytes, so creating
//! an empty file or replacing a file with a directory both change the digest.

use std::io;
use std::path::{Path, PathBuf};

use indexmap::{IndexMap, IndexSet};
use sha2::{Digest, Sha256};

use crate::errors::Result;
use crate::model::catalog::{HOOKS_DIRNAME, MCPS_DIRNAME, PACKS_DIRNAME, SKILLS_DIRNAME};
use crate::model::config::{CONFIG_FILENAMES, ProjectConfig};
use crate::model::lock_file::LOCK_FILENAME;
use crate::model::sources::{Source, SourceRequest, parse_source};
use crate::model::state::{STATE_DIRNAME, STATE_FILENAME};
use crate::project::gitignore::{GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE};
use crate::util::cmp::js_cmp;
use crate::util::fs::{EntryKind, canonicalize, lstat_kind, read_dir_names};
use crate::util::hash::hex;
use crate::util::path::{join, resolve};

/// The digest of a setup's inputs at one moment. Two fingerprints are equal exactly when nothing
/// covered changed between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupFingerprint {
    /// SHA-256 hex over the project files.
    inputs: String,
    /// SHA-256 hex over each `path:` catalog's contents, by catalog name, in config order.
    local_catalogs: IndexMap<String, String>,
}

impl SetupFingerprint {
    /// The `path:` catalogs whose contents differ between `self` and `other`, by name: what to
    /// name when a review goes stale because a local catalog was edited.
    pub fn changed_catalogs(&self, other: &Self) -> Vec<String> {
        let names: IndexSet<&String> = self
            .local_catalogs
            .keys()
            .chain(other.local_catalogs.keys())
            .collect();

        names
            .into_iter()
            .filter(|name| self.local_catalogs.get(*name) != other.local_catalogs.get(*name))
            .cloned()
            .collect()
    }

    /// Whether the project files (config, lock, state, gitignores) differ between the two.
    pub fn inputs_changed(&self, other: &Self) -> bool {
        self.inputs != other.inputs
    }
}

/// Feeds one length-prefixed field, so two fields can never run together into a third.
fn field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

/// Feeds the state of the file at `path`: absent, not a regular file, unreadable, or its bytes.
fn file_state(hasher: &mut Sha256, path: &Path) {
    match lstat_kind(path) {
        Ok(EntryKind::Missing) => field(hasher, b"absent"),
        Ok(EntryKind::File | EntryKind::Symlink) => match std::fs::read(path) {
            Ok(bytes) => {
                field(hasher, b"file");
                field(hasher, &bytes);
            }
            Err(error) => field(hasher, format!("unreadable {:?}", error.kind()).as_bytes()),
        },
        Ok(EntryKind::Dir | EntryKind::Other) => field(hasher, b"not a file"),
        Err(error) => field(hasher, format!("unreadable {:?}", error.kind()).as_bytes()),
    }
}

/// Feeds every entry under `dir`, recursively, in [`js_cmp`] order.
///
/// A symlink contributes where it points and is then followed. `visiting` holds the canonical
/// directories on the current path, so a link back up the tree is hashed as a cycle instead of
/// walked forever.
fn tree(hasher: &mut Sha256, dir: &Path, relative: &str, visiting: &mut Vec<PathBuf>) {
    let canonical = canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());

    if visiting.contains(&canonical) {
        field(hasher, format!("cycle {relative}").as_bytes());
        return;
    }

    let mut names = match read_dir_names(dir) {
        Ok(names) => names,
        Err(error) => {
            field(
                hasher,
                format!("unlistable {relative} {:?}", error.kind()).as_bytes(),
            );
            return;
        }
    };

    names.sort_by(|a, b| js_cmp(a, b));
    visiting.push(canonical);

    for name in names {
        let path = dir.join(&name);
        let within = format!("{relative}/{name}");

        field(hasher, within.as_bytes());
        entry(hasher, &path, &within, visiting);
    }

    visiting.pop();
}

/// Feeds one entry of a catalog tree.
fn entry(hasher: &mut Sha256, path: &Path, relative: &str, visiting: &mut Vec<PathBuf>) {
    match lstat_kind(path) {
        Ok(EntryKind::Dir) => tree(hasher, path, relative, visiting),
        Ok(EntryKind::Symlink) => {
            let target = std::fs::read_link(path)
                .map(|target| target.to_string_lossy().into_owned())
                .unwrap_or_default();

            field(hasher, b"link");
            field(hasher, target.as_bytes());

            match std::fs::metadata(path) {
                Ok(metadata) if metadata.is_dir() => tree(hasher, path, relative, visiting),
                Ok(_) => file_state(hasher, path),
                Err(error) if error.kind() == io::ErrorKind::NotFound => field(hasher, b"dangling"),
                Err(error) => field(hasher, format!("unreadable {:?}", error.kind()).as_bytes()),
            }
        }
        _ => file_state(hasher, path),
    }
}

/// The digest of one `path:` catalog's item directories.
fn catalog_digest(root: &Path) -> String {
    let mut hasher = Sha256::new();

    for dirname in [SKILLS_DIRNAME, MCPS_DIRNAME, HOOKS_DIRNAME, PACKS_DIRNAME] {
        let dir = join(root, dirname);

        field(&mut hasher, dirname.as_bytes());
        entry(&mut hasher, &dir, dirname, &mut Vec::new());
    }

    hex(&hasher.finalize())
}

/// Fingerprints the setup at `root` as `config` would install it.
///
/// `config` names the `path:` catalogs to hash: the reviewed one, which need not be the one on
/// disk. A source that does not parse is skipped; the review reports it.
///
/// # Errors
///
/// None today: anything unreadable is hashed as unreadable rather than failing, so it reads as a
/// change if it later becomes readable. The `Result` leaves room for a failure that should stop
/// an apply.
#[allow(clippy::unnecessary_wraps)]
pub fn setup_fingerprint(root: &Path, config: Option<&ProjectConfig>) -> Result<SetupFingerprint> {
    let mut hasher = Sha256::new();
    let state = format!("{STATE_DIRNAME}/{STATE_FILENAME}");

    for file in CONFIG_FILENAMES.iter().copied().chain([
        LOCK_FILENAME,
        state.as_str(),
        GITIGNORE_FILENAME,
        SHARED_GITIGNORE_FILE,
    ]) {
        field(&mut hasher, file.as_bytes());
        file_state(&mut hasher, &join(root, file));
    }

    let mut local_catalogs = IndexMap::new();

    for catalog in config
        .map(|config| config.catalogs.as_slice())
        .unwrap_or_default()
    {
        let request = SourceRequest {
            source: catalog.source.clone(),
            r#ref: catalog.r#ref.clone(),
            ..SourceRequest::default()
        };

        if let Ok(Source::Path { directory }) = parse_source(&request) {
            local_catalogs.insert(
                catalog.name.clone(),
                catalog_digest(&resolve(root, &directory)),
            );
        }
    }

    Ok(SetupFingerprint {
        inputs: hex(&hasher.finalize()),
        local_catalogs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::parse_project_config;
    use crate::test_support::tempdir;
    use crate::util::fs::{mkdir_p, write_text};

    fn config() -> ProjectConfig {
        parse_project_config(
            "version: 1\ncatalogs:\n  - name: local\n    source: path:./catalog\nrequires: []\n",
            "ambit.yml",
        )
        .unwrap()
    }

    fn write(root: &Path, relative: &str, text: &str) {
        let path = join(root, relative);

        mkdir_p(path.parent().unwrap()).unwrap();
        write_text(&path, text).unwrap();
    }

    #[test]
    fn is_stable_while_nothing_changes() {
        let dir = tempdir();

        write(dir.path(), "ambit.yml", "version: 1\n");
        write(dir.path(), "catalog/skills/a/SKILL.md", "a");

        let config = config();

        assert_eq!(
            setup_fingerprint(dir.path(), Some(&config)).unwrap(),
            setup_fingerprint(dir.path(), Some(&config)).unwrap()
        );
    }

    #[test]
    fn changes_with_each_covered_project_file() {
        for file in [
            "ambit.yml",
            "ambit.yaml",
            "ambit.lock",
            ".ambit/state.json",
            ".gitignore",
            ".agents/.gitignore",
        ] {
            let dir = tempdir();
            let before = setup_fingerprint(dir.path(), None).unwrap();

            write(dir.path(), file, "");

            let after = setup_fingerprint(dir.path(), None).unwrap();

            assert!(before.inputs_changed(&after), "{file}");
        }
    }

    #[test]
    fn names_a_local_catalog_whose_contents_changed() {
        let dir = tempdir();
        let config = config();

        write(dir.path(), "catalog/skills/a/SKILL.md", "a");

        let before = setup_fingerprint(dir.path(), Some(&config)).unwrap();

        write(dir.path(), "catalog/skills/a/SKILL.md", "b");

        let after = setup_fingerprint(dir.path(), Some(&config)).unwrap();

        assert!(!before.inputs_changed(&after));
        assert_eq!(before.changed_catalogs(&after), ["local"]);
    }

    #[test]
    fn ignores_files_outside_the_item_directories() {
        let dir = tempdir();
        let config = config();
        let before = setup_fingerprint(dir.path(), Some(&config)).unwrap();

        write(dir.path(), "catalog/README.md", "notes");

        assert_eq!(
            before,
            setup_fingerprint(dir.path(), Some(&config)).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn survives_a_symlink_cycle() {
        let dir = tempdir();
        let config = config();

        write(dir.path(), "catalog/skills/a/SKILL.md", "a");
        std::os::unix::fs::symlink("..", dir.path().join("catalog/skills/a/up")).unwrap();

        assert_eq!(
            setup_fingerprint(dir.path(), Some(&config)).unwrap(),
            setup_fingerprint(dir.path(), Some(&config)).unwrap()
        );
    }
}
