#![allow(clippy::disallowed_methods)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::util::path::normalize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Missing,
    Symlink,
    Dir,
    File,
    Other,
}

pub fn read_text(p: &Path) -> io::Result<String> {
    let bytes = fs::read(p)?;

    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    })
}

pub fn read_text_opt(p: &Path) -> io::Result<Option<String>> {
    match read_text(p) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn read_dir_names(dir: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();

    for entry in fs::read_dir(dir)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }

    #[cfg(test)]
    read_order::apply(dir, &mut names);

    Ok(names)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub relative: String,
    pub kind: EntryKind,
}

pub fn walk_tree(dir: &Path) -> io::Result<Vec<TreeEntry>> {
    fn walk(current: &Path, relative: &str, found: &mut Vec<TreeEntry>) -> io::Result<()> {
        for name in read_dir_names(current)? {
            let within = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let entry = current.join(&name);
            let kind = lstat_kind(&entry)?;

            if kind == EntryKind::Dir {
                walk(&entry, &within, found)?;
            } else {
                found.push(TreeEntry {
                    relative: within,
                    kind,
                });
            }
        }

        Ok(())
    }

    let mut found = Vec::new();

    walk(dir, "", &mut found)?;
    found.sort_by(|a, b| crate::util::cmp::js_cmp(&a.relative, &b.relative));
    Ok(found)
}

pub fn read_link_text(p: &Path) -> io::Result<String> {
    Ok(crate::util::path::to_slash(&fs::read_link(p)?))
}

pub fn rm_rf(p: &Path) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(p) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    let result = if metadata.is_dir() {
        fs::remove_dir_all(p)
    } else {
        remove_link_or_file(p, &metadata)
    };

    match result {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

#[cfg(windows)]
fn remove_link_or_file(p: &Path, metadata: &fs::Metadata) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;

    if metadata.file_type().is_symlink_dir() {
        fs::remove_dir(p)
    } else {
        fs::remove_file(p)
    }
}

#[cfg(not(windows))]
fn remove_link_or_file(p: &Path, _metadata: &fs::Metadata) -> io::Result<()> {
    fs::remove_file(p)
}

pub fn rmdir_if_empty(p: &Path) -> io::Result<()> {
    match fs::remove_dir(p) {
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        other => other,
    }
}

pub fn mkdir_p(p: &Path) -> io::Result<()> {
    fs::create_dir_all(p)
}

pub fn write_text(p: &Path, text: &str) -> io::Result<()> {
    fs::write(p, text)
}

// In-tree relative links are kept verbatim so the copy has the same `tree_digest` as its source.
pub fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    copy_entry(&normalize(src), src, dst)
}

fn copy_entry(root: &Path, src: &Path, dst: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(src)?;
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        let target = fs::read_link(src)?;
        let resolved = if target.has_root() {
            target.clone()
        } else {
            let parent = src.parent().unwrap_or_else(|| Path::new(""));
            normalize(&parent.join(&target))
        };
        let inside = !target.has_root() && resolved != root && resolved.starts_with(root);
        let written = if inside { target } else { resolved.clone() };

        if fs::symlink_metadata(dst).is_ok() {
            rm_rf(dst)?;
        }

        return if fs::metadata(&resolved).is_ok_and(|m| m.is_dir()) {
            symlink_dir(&written, dst)
        } else {
            symlink_file(&written, dst)
        };
    }

    if file_type.is_dir() {
        fs::create_dir_all(dst)?;

        let mut names = read_dir_names(src)?;
        names.sort_by(|a, b| crate::util::cmp::js_cmp(a, b));

        for name in names {
            copy_entry(root, &src.join(&name), &dst.join(&name))?;
        }

        fs::set_permissions(dst, metadata.permissions())?;
        return Ok(());
    }

    fs::copy(src, dst)?;
    Ok(())
}

pub fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);

    #[cfg(windows)]
    return std::os::windows::fs::symlink_dir(target, link);
}

pub fn symlink_file(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);

    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link);
}

pub fn lstat_kind(p: &Path) -> io::Result<EntryKind> {
    match fs::symlink_metadata(p) {
        Ok(metadata) => {
            let file_type = metadata.file_type();

            Ok(if file_type.is_symlink() {
                EntryKind::Symlink
            } else if file_type.is_dir() {
                EntryKind::Dir
            } else if file_type.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(EntryKind::Missing),
        Err(error) => Err(error),
    }
}

pub fn io_message(err: &io::Error, path: &Path) -> String {
    format!("{}: {err}", path.display())
}

pub fn canonicalize(p: &Path) -> io::Result<PathBuf> {
    let resolved = fs::canonicalize(p)?;

    if cfg!(windows) {
        let text = resolved.to_string_lossy();

        // Dropped because git cannot work under a verbatim `\\?\` path.
        if let Some(plain) = text.strip_prefix(r"\\?\")
            && !plain.starts_with(r"UNC\")
        {
            return Ok(PathBuf::from(plain));
        }
    }

    Ok(resolved)
}

#[cfg(test)]
pub mod read_order {
    use std::cell::{Cell, RefCell};
    use std::path::{Path, PathBuf};

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum ReadOrder {
        Natural,
        Reversed,
        Rotated,
    }

    thread_local! {
        static READ_ORDER: Cell<ReadOrder> = const { Cell::new(ReadOrder::Natural) };
        static SEEN: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn apply(dir: &Path, names: &mut [String]) {
        SEEN.with(|seen| seen.borrow_mut().push(dir.to_path_buf()));

        match READ_ORDER.with(Cell::get) {
            ReadOrder::Natural => {}
            ReadOrder::Reversed => names.reverse(),
            ReadOrder::Rotated => {
                if !names.is_empty() {
                    names.rotate_left(1);
                }
            }
        }
    }

    pub fn with_read_order<R>(order: ReadOrder, f: impl FnOnce() -> R) -> R {
        struct Restore(ReadOrder);

        impl Drop for Restore {
            fn drop(&mut self) {
                READ_ORDER.with(|cell| cell.set(self.0));
            }
        }

        let _restore = Restore(READ_ORDER.with(|cell| cell.replace(order)));
        f()
    }

    pub fn take_seen() -> Vec<PathBuf> {
        SEEN.with(|seen| std::mem::take(&mut *seen.borrow_mut()))
    }
}

#[cfg(test)]
mod tests {
    use super::read_order::{ReadOrder, take_seen, with_read_order};
    use super::*;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn reads_absent_files_as_none_and_invalid_utf8_lossily() {
        let dir = tempdir();
        let file = dir.path().join("f");

        assert_eq!(read_text_opt(&file).unwrap(), None);
        fs::write(&file, b"a\xffb").unwrap();
        assert_eq!(read_text_opt(&file).unwrap().as_deref(), Some("a\u{FFFD}b"));
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "Windows has no ENOTDIR: a path under a file is ERROR_PATH_NOT_FOUND, which is absence"
    )]
    fn treats_not_a_directory_as_an_error() {
        let dir = tempdir();
        let file = dir.path().join("f");
        fs::write(&file, "x").unwrap();

        assert!(read_text_opt(&file.join("child")).is_err());
    }

    #[test]
    fn permutes_listings_per_thread() {
        let dir = tempdir();
        for name in ["a", "b", "c"] {
            fs::write(dir.path().join(name), "").unwrap();
        }

        let mut natural = read_dir_names(dir.path()).unwrap();
        let reversed = with_read_order(ReadOrder::Reversed, || read_dir_names(dir.path()).unwrap());
        let rotated = with_read_order(ReadOrder::Rotated, || read_dir_names(dir.path()).unwrap());

        let mut expected = natural.clone();
        expected.reverse();
        assert_eq!(reversed, expected);
        natural.rotate_left(1);
        assert_eq!(rotated, natural);
        assert_eq!(take_seen().len(), 3);
    }

    #[test]
    fn removes_files_dirs_and_absent_paths() {
        let dir = tempdir();
        let tree = dir.path().join("t");
        mkdir_p(&tree.join("x/y")).unwrap();
        write_text(&tree.join("x/y/f"), "z").unwrap();
        let file = dir.path().join("f");
        write_text(&file, "z").unwrap();

        rm_rf(&tree).unwrap();
        rm_rf(&file).unwrap();
        rm_rf(&dir.path().join("absent")).unwrap();

        assert_eq!(lstat_kind(&tree).unwrap(), EntryKind::Missing);
        assert_eq!(lstat_kind(&file).unwrap(), EntryKind::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn copies_trees_rewriting_relative_symlinks() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir();
        let src = dir.path().join("src");
        mkdir_p(&src.join("sub")).unwrap();
        write_text(&src.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        write_text(&dir.path().join("outside"), "o").unwrap();
        symlink_file(Path::new("../outside"), &src.join("link")).unwrap();

        let dst = dir.path().join("dst");
        copy_tree(&src, &dst).unwrap();

        let mode = fs::metadata(dst.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
        assert_eq!(lstat_kind(&dst.join("sub")).unwrap(), EntryKind::Dir);
        assert_eq!(lstat_kind(&dst.join("link")).unwrap(), EntryKind::Symlink);
        assert_eq!(
            fs::read_link(dst.join("link")).unwrap(),
            dir.path().join("outside")
        );
    }

    #[cfg(unix)]
    #[test]
    fn copies_links_inside_the_tree_verbatim() {
        let dir = tempdir();
        let src = dir.path().join("src");
        mkdir_p(&src.join("sub")).unwrap();
        write_text(&src.join("data.txt"), "d").unwrap();
        symlink_file(Path::new("../data.txt"), &src.join("sub/link")).unwrap();

        let dst = dir.path().join("dst");
        copy_tree(&src, &dst).unwrap();

        assert_eq!(
            read_link_text(&dst.join("sub/link")).unwrap(),
            "../data.txt"
        );
        assert_eq!(read_text(&dst.join("sub/link")).unwrap(), "d");
    }

    #[cfg(unix)]
    #[test]
    fn walks_files_and_links_in_order_skipping_empty_directories() {
        let dir = tempdir();
        mkdir_p(&dir.path().join("b/empty")).unwrap();
        write_text(&dir.path().join("b/z"), "").unwrap();
        write_text(&dir.path().join("a"), "").unwrap();
        symlink_file(Path::new("a"), &dir.path().join("c")).unwrap();

        let entries = with_read_order(ReadOrder::Reversed, || walk_tree(dir.path()).unwrap());
        let listed: Vec<(&str, EntryKind)> = entries
            .iter()
            .map(|entry| (entry.relative.as_str(), entry.kind))
            .collect();

        assert_eq!(
            listed,
            [
                ("a", EntryKind::File),
                ("b/z", EntryKind::File),
                ("c", EntryKind::Symlink)
            ]
        );
    }

    #[test]
    fn prefixes_errors_with_their_path() {
        let error = io::Error::other("boom");

        assert_eq!(io_message(&error, Path::new("/x/y")), "/x/y: boom");
    }
}
