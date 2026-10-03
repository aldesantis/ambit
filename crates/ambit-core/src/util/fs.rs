//! Filesystem calls with Node's semantics, which ambit's behaviour and error text follow.
//!
//! - Only `NotFound` means "absent". Any other failure (`ENOTDIR`, `EACCES`) is an error, because
//!   "I could not look" is not the same answer as "nothing is there".
//! - Text reads decode invalid UTF-8 lossily, as `readFile(…, "utf8")` does.
//! - [`read_dir_names`] is the only directory listing in ambit. Under `cfg(test)` it is permuted
//!   by a per-thread hook, which is how the determinism suite proves no output depends on the
//!   order the OS lists a directory in.
#![allow(clippy::disallowed_methods)]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::util::path::normalize;

/// What `lstat` found at a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Missing,
    Symlink,
    Dir,
    File,
    Other,
}

/// The whole file as text, invalid UTF-8 replaced, like `readFile(path, "utf8")`.
///
/// # Errors
///
/// Any I/O error, `NotFound` included.
pub fn read_text(p: &Path) -> io::Result<String> {
    let bytes = fs::read(p)?;

    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    })
}

/// [`read_text`], with an absent file as `None`.
///
/// # Errors
///
/// Any I/O error other than `NotFound`.
pub fn read_text_opt(p: &Path) -> io::Result<Option<String>> {
    match read_text(p) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// The names in a directory, in the order the OS lists them.
///
/// Callers that need an order sort the result; the determinism suite permutes it under
/// `cfg(test)` to prove they do.
///
/// # Errors
///
/// Any I/O error, `NotFound` included.
pub fn read_dir_names(dir: &Path) -> io::Result<Vec<String>> {
    let mut names = Vec::new();

    for entry in fs::read_dir(dir)? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }

    #[cfg(test)]
    read_order::apply(dir, &mut names);

    Ok(names)
}

/// `fs.rm(p, { recursive: true, force: true })`: removes a file, a symlink (never following it), or
/// a directory tree. An absent path is not an error.
///
/// # Errors
///
/// Any I/O error other than `NotFound`.
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

/// `fs.mkdir(p, { recursive: true })`.
///
/// # Errors
///
/// Any I/O error.
pub fn mkdir_p(p: &Path) -> io::Result<()> {
    fs::create_dir_all(p)
}

/// `fs.writeFile(p, text)`.
///
/// # Errors
///
/// Any I/O error.
pub fn write_text(p: &Path, text: &str) -> io::Result<()> {
    fs::write(p, text)
}

/// `fs.writeFile(p, text, { flag: "wx" })`: writes a file that must not exist yet.
///
/// # Errors
///
/// `AlreadyExists` when something is already at `p`; any other I/O error.
pub fn write_new(p: &Path, text: &str) -> io::Result<()> {
    use std::io::Write as _;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)?;

    file.write_all(text.as_bytes())
}

/// `fs.rename(from, to)`: replaces `to` atomically when both are on one filesystem.
///
/// # Errors
///
/// Any I/O error.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

/// Opens `p` (creating the file, but not its directory) and takes an exclusive OS lock on it
/// without waiting.
///
/// `Ok(None)` means another open file holds the lock: another process, or another handle in this
/// one. The lock lasts as long as the returned [`FileLock`] and is released by the OS if the
/// process dies. The file is never truncated or deleted here: deleting a lock file while another
/// process waits on it would let two holders each lock a different inode.
///
/// # Errors
///
/// Any I/O error opening or locking the file.
pub fn try_lock_file(p: &Path) -> io::Result<Option<FileLock>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(p)?;

    match file.try_lock() {
        Ok(()) => Ok(Some(FileLock { file })),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => Err(error),
    }
}

/// An OS lock taken by [`try_lock_file`], released when dropped.
///
/// Released by an explicit unlock rather than by closing the file. A `flock` lock belongs to the
/// open file description, which a child forked by any thread of this process shares until its
/// `exec` closes the descriptor; closing ours alone would leave the lock held for that window.
/// `std::process::Command` forks rather than spawns whenever the child's `PATH` is overridden,
/// which is how every git child is started.
#[derive(Debug)]
pub struct FileLock {
    file: fs::File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

/// `fs.cp(src, dst, { recursive: true })`.
///
/// Files keep their permissions. A symlink is recreated rather than followed, and a relative target
/// is first resolved against the source link's own directory, as Node does without
/// `verbatimSymlinks`, so the copy still points at what the original pointed at. Existing files at
/// the destination are overwritten.
///
/// # Errors
///
/// Any I/O error.
pub fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(src)?;
    let file_type = metadata.file_type();

    if file_type.is_symlink() {
        let target = fs::read_link(src)?;
        let resolved = if target.has_root() {
            target
        } else {
            let parent = src.parent().unwrap_or_else(|| Path::new(""));
            normalize(&parent.join(target))
        };

        if fs::symlink_metadata(dst).is_ok() {
            rm_rf(dst)?;
        }

        return if fs::metadata(&resolved).is_ok_and(|m| m.is_dir()) {
            symlink_dir(&resolved, dst)
        } else {
            symlink_file(&resolved, dst)
        };
    }

    if file_type.is_dir() {
        fs::create_dir_all(dst)?;

        let mut names = read_dir_names(src)?;
        names.sort_by(|a, b| crate::util::cmp::js_cmp(a, b));

        for name in names {
            copy_tree(&src.join(&name), &dst.join(&name))?;
        }

        fs::set_permissions(dst, metadata.permissions())?;
        return Ok(());
    }

    fs::copy(src, dst)?;
    Ok(())
}

/// A symlink at `link` pointing at the directory `target`, like `symlink(target, link, "dir")`.
///
/// # Errors
///
/// Any I/O error. On Windows that includes lacking the privilege to create a symlink.
pub fn symlink_dir(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);

    #[cfg(windows)]
    return std::os::windows::fs::symlink_dir(target, link);
}

/// A symlink at `link` pointing at the file `target`.
///
/// # Errors
///
/// Any I/O error.
pub fn symlink_file(target: &Path, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);

    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link);
}

/// What `lstat` finds at `p`, with an absent path as [`EntryKind::Missing`].
///
/// # Errors
///
/// Any I/O error other than `NotFound`.
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

/// An I/O error worded as Node words it: `ENOENT: no such file or directory, open '<path>'`.
///
/// Errors Node has no code for fall back to Rust's own message.
pub fn io_message(err: &io::Error, syscall: &str, path: &Path) -> String {
    match node_code(err) {
        Some((code, text)) => format!("{code}: {text}, {syscall} '{}'", path.display()),
        None => err.to_string(),
    }
}

fn node_code(err: &io::Error) -> Option<(&'static str, &'static str)> {
    use io::ErrorKind as K;

    // EPERM and EACCES share a kind; the raw errno (1 and 13 on every Unix ambit ships for) tells
    // them apart.
    if cfg!(unix) && err.raw_os_error() == Some(1) {
        return Some(("EPERM", "operation not permitted"));
    }

    Some(match err.kind() {
        K::NotFound => ("ENOENT", "no such file or directory"),
        K::PermissionDenied => ("EACCES", "permission denied"),
        K::AlreadyExists => ("EEXIST", "file already exists"),
        K::NotADirectory => ("ENOTDIR", "not a directory"),
        K::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        K::DirectoryNotEmpty => ("ENOTEMPTY", "directory not empty"),
        K::ReadOnlyFilesystem => ("EROFS", "read-only file system"),
        K::CrossesDevices => ("EXDEV", "cross-device link not permitted"),
        K::ResourceBusy => ("EBUSY", "resource busy or locked"),
        K::StorageFull => ("ENOSPC", "no space left on device"),
        _ => return None,
    })
}

/// The canonical form of `p` when it exists, for comparing two paths that may differ only by a
/// symlink (macOS's `/var` and `/private/var`).
///
/// On Windows the verbatim `\\?\` prefix is dropped from a drive path. The plain spelling names the
/// same file, is the one a user recognizes in a message, and is one git can work under.
///
/// # Errors
///
/// Any I/O error.
pub fn canonicalize(p: &Path) -> io::Result<PathBuf> {
    let resolved = fs::canonicalize(p)?;

    if cfg!(windows) {
        let text = resolved.to_string_lossy();

        if let Some(plain) = text.strip_prefix(r"\\?\")
            && !plain.starts_with(r"UNC\")
        {
            return Ok(PathBuf::from(plain));
        }
    }

    Ok(resolved)
}

/// The determinism hook: a per-thread permutation of [`read_dir_names`]'s result.
#[cfg(test)]
pub mod read_order {
    use std::cell::{Cell, RefCell};
    use std::path::{Path, PathBuf};

    /// How the current thread's directory listings are permuted.
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

    /// Runs `f` with this thread's listings permuted by `order`, restoring the previous order after.
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

    /// Every directory this thread listed since the last call, in listing order.
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

    #[test]
    fn words_errors_like_node() {
        let error = io::Error::from(io::ErrorKind::NotFound);

        assert_eq!(
            io_message(&error, "open", Path::new("/x/y")),
            "ENOENT: no such file or directory, open '/x/y'"
        );
    }

    #[test]
    fn writes_a_new_file_and_refuses_an_existing_one() {
        let dir = tempdir();
        let file = dir.path().join("f");

        write_new(&file, "one").unwrap();
        let error = write_new(&file, "two").unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(read_text(&file).unwrap(), "one");
    }

    #[test]
    fn a_held_lock_refuses_a_second_locker_until_dropped() {
        let dir = tempdir();
        let file = dir.path().join("lock");
        let first = try_lock_file(&file).unwrap().expect("the first lock");

        let contender = file.clone();
        let refused = std::thread::spawn(move || try_lock_file(&contender).unwrap().is_none())
            .join()
            .unwrap();

        assert!(refused);

        drop(first);

        assert!(try_lock_file(&file).unwrap().is_some());
        assert!(file.exists(), "the lock file is never deleted");
    }

    #[test]
    fn dropping_a_lock_releases_it_while_a_duplicate_descriptor_stays_open() {
        let dir = tempdir();
        let file = dir.path().join("lock");
        let lock = try_lock_file(&file).unwrap().expect("the first lock");
        // What a child forked between our open and its exec holds: the same open file description.
        let inherited = lock.file.try_clone().unwrap();

        drop(lock);

        assert!(try_lock_file(&file).unwrap().is_some());
        drop(inherited);
    }
}
