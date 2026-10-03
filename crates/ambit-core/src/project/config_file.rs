//! Writing `ambit.yml` atomically, refusing to overwrite an edit nobody reviewed.
//!
//! The text goes to a temporary file beside the config first, is flushed to disk, and only then
//! takes the config's name, so a reader (another ambit, an editor) sees the old file or the new
//! one and never half of either. A new config takes its name by hard link, which fails when the
//! name already exists; an existing one is compared with what the caller last read and replaced by
//! rename. Between that comparison and the rename an external writer can still slip in: the
//! window is a few syscalls, and the setup lock keeps other ambit operations out of it.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::errors::{AmbitError, Result, config_error};
use crate::model::config::CONFIG_FILENAMES;
use crate::project::review::stale_review;
use crate::util::fs::{io_message, rename};
use crate::util::path::join;

/// The error for a config that could not be written. Exit 2.
fn save_failed(file: &str, error: &io::Error, syscall: &str, path: &Path) -> AmbitError {
    config_error(
        format!("cannot save {file}"),
        [
            io_message(error, syscall, path),
            format!(
                "make {} writable, then apply the changes again",
                path.parent().unwrap_or(path).display()
            ),
        ],
    )
}

/// A temporary file in `root`, named after `file` and unique to this process and moment.
fn temp_path(root: &Path, file: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());

    join(root, &format!(".{file}.{}-{nanos}.tmp", std::process::id()))
}

/// The bytes at `path`, `None` when nothing is there.
fn read_opt(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Writes `text` to a new file at `temp`, flushed to disk, with `like`'s permissions when given.
fn write_temp(temp: &Path, text: &str, like: Option<&Path>) -> io::Result<()> {
    let mut handle: File = OpenOptions::new().write(true).create_new(true).open(temp)?;

    handle.write_all(text.as_bytes())?;

    if let Some(like) = like {
        handle.set_permissions(fs::metadata(like)?.permissions())?;
    }

    handle.sync_all()
}

/// Saves `text` as the config file `file` in `root`, atomically.
///
/// `expected` is the config text the caller reviewed against: `None` when there was no config, in
/// which case neither accepted config name may exist now; otherwise `file` must still hold exactly
/// those bytes. The new file keeps the old one's permissions. Nothing is left behind on failure:
/// the temporary file is always removed.
///
/// # Errors
///
/// Exit 2: [`STALE_REVIEW`](crate::project::review::STALE_REVIEW) when the config on disk is not
/// what `expected` says, or another config name appeared; a save failure naming the file and the
/// failing call otherwise. `file` must be one of the accepted config names, or the error is exit 1.
pub fn save_config_atomic(
    root: &Path,
    file: &str,
    expected: Option<&str>,
    text: &str,
) -> Result<()> {
    if !CONFIG_FILENAMES.contains(&file) {
        return Err(AmbitError::unexpected(format!(
            "\"{file}\" is not a config filename"
        )));
    }

    let target = join(root, file);

    for other in CONFIG_FILENAMES.iter().filter(|&&other| other != file) {
        let path = join(root, other);

        if read_opt(&path)
            .map_err(|error| save_failed(file, &error, "open", &path))?
            .is_some()
        {
            return Err(stale_review(format!("{other} appeared beside {file}")));
        }
    }

    let temp = temp_path(root, file);
    let like = expected.map(|_| target.as_path());
    let result = write_temp(&temp, text, like)
        .map_err(|error| save_failed(file, &error, "write", &temp))
        .and_then(|()| commit(&temp, &target, file, expected));

    // Already gone after a rename; a leftover after any failure is removed. Neither outcome
    // depends on this succeeding.
    let _ = fs::remove_file(&temp);

    result
}

/// Gives the written temporary file the config's name.
fn commit(temp: &Path, target: &Path, file: &str, expected: Option<&str>) -> Result<()> {
    let current = read_opt(target).map_err(|error| save_failed(file, &error, "open", target))?;

    let Some(expected) = expected else {
        if current.is_some() {
            return Err(stale_review(format!("{file} was created by someone else")));
        }

        return match fs::hard_link(temp, target) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                Err(stale_review(format!("{file} was created by someone else")))
            }
            Err(error) => Err(save_failed(file, &error, "link", target)),
        };
    };

    if current.as_deref() != Some(expected.as_bytes()) {
        return Err(stale_review(format!("{file} changed on disk")));
    }

    rename(temp, target).map_err(|error| save_failed(file, &error, "rename", target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ExitCode;
    use crate::project::review::STALE_REVIEW;
    use crate::test_support::tempdir;
    use crate::util::fs::{read_dir_names, read_text, write_text};

    fn names(root: &Path) -> Vec<String> {
        let mut names = read_dir_names(root).unwrap();

        names.sort();
        names
    }

    #[test]
    fn creates_a_new_config() {
        let dir = tempdir();

        save_config_atomic(dir.path(), "ambit.yml", None, "version: 1\n").unwrap();

        assert_eq!(
            read_text(&dir.path().join("ambit.yml")).unwrap(),
            "version: 1\n"
        );
        assert_eq!(names(dir.path()), ["ambit.yml"]);
    }

    #[test]
    fn replaces_an_existing_config_it_was_shown() {
        let dir = tempdir();
        let target = dir.path().join("ambit.yaml");

        write_text(&target, "old\n").unwrap();
        save_config_atomic(dir.path(), "ambit.yaml", Some("old\n"), "new\n").unwrap();

        assert_eq!(read_text(&target).unwrap(), "new\n");
        assert_eq!(names(dir.path()), ["ambit.yaml"]);
    }

    #[test]
    fn refuses_a_config_edited_since_it_was_read() {
        let dir = tempdir();
        let target = dir.path().join("ambit.yml");

        write_text(&target, "edited\n").unwrap();

        let error =
            save_config_atomic(dir.path(), "ambit.yml", Some("old\n"), "new\n").unwrap_err();

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(error.message, STALE_REVIEW);
        assert_eq!(read_text(&target).unwrap(), "edited\n");
        assert_eq!(names(dir.path()), ["ambit.yml"]);
    }

    #[test]
    fn refuses_a_config_created_since_it_was_missing() {
        let dir = tempdir();

        write_text(&dir.path().join("ambit.yml"), "theirs\n").unwrap();

        let error = save_config_atomic(dir.path(), "ambit.yml", None, "mine\n").unwrap_err();

        assert_eq!(error.message, STALE_REVIEW);
        assert_eq!(
            read_text(&dir.path().join("ambit.yml")).unwrap(),
            "theirs\n"
        );
    }

    #[test]
    fn refuses_to_create_a_second_config_name() {
        let dir = tempdir();

        write_text(&dir.path().join("ambit.yaml"), "theirs\n").unwrap();

        let error = save_config_atomic(dir.path(), "ambit.yml", None, "mine\n").unwrap_err();

        assert_eq!(error.message, STALE_REVIEW);
        assert_eq!(names(dir.path()), ["ambit.yaml"]);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_existing_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir();
        let target = dir.path().join("ambit.yml");

        write_text(&target, "old\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        save_config_atomic(dir.path(), "ambit.yml", Some("old\n"), "new\n").unwrap();

        let mode = fs::metadata(&target).unwrap().permissions().mode() & 0o777;

        assert_eq!(mode, 0o640);
    }
}
