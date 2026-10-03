//! The per-project operation lock: one mutating ambit operation at a time in a project, whether it
//! comes from the CLI or the app.
//!
//! An OS file lock on [`OPERATION_LOCK_FILENAME`] inside `.ambit/`, not a lock directory or a PID
//! file, so a crashed holder releases it without anyone cleaning up. The file is created on first
//! use and never deleted: removing a lock file another process has open would let a third process
//! lock a new file at the same path while the first still holds the old one. A contended lock fails
//! at once rather than waiting, since the other operation may be a long install.

use std::path::{Path, PathBuf};

use crate::errors::{AmbitError, ExitCode, Result, config_error};
use crate::model::state::STATE_DIRNAME;
use crate::util::fs::{FileLock, io_message, mkdir_p, try_lock_file};
use crate::util::path::join;

/// The lock file's name inside `.ambit/`.
pub const OPERATION_LOCK_FILENAME: &str = "operation.lock";

/// How the error for a held lock begins, so a library caller can recognize it.
pub const OPERATION_IN_PROGRESS: &str = "another ambit operation is using";

/// Proof that this process holds a project's operation lock. Released on drop.
#[derive(Debug)]
pub struct SetupLock {
    _file: FileLock,
}

impl SetupLock {
    /// Takes the operation lock of the project at `root`, creating `.ambit/` and the lock file
    /// when missing. `root` itself must exist.
    ///
    /// # Errors
    ///
    /// Exit 2 when another operation holds the lock ([`OPERATION_IN_PROGRESS`]), when `root` is
    /// not a directory, or when the lock file cannot be created.
    pub fn acquire(root: &Path) -> Result<Self> {
        let path = operation_lock_path(root);
        let cannot = |message: String| {
            config_error(
                format!("cannot lock {}", root.display()),
                [
                    message,
                    format!("check that {} is writable", root.display()),
                ],
            )
        };

        if !std::fs::metadata(root).is_ok_and(|metadata| metadata.is_dir()) {
            return Err(config_error(
                format!("cannot lock {}", root.display()),
                [
                    "it is not a directory".to_owned(),
                    "point the command at a project directory that exists".to_owned(),
                ],
            ));
        }

        let dir = join(root, STATE_DIRNAME);
        mkdir_p(&dir).map_err(|error| cannot(io_message(&error, "mkdir", &dir)))?;

        match try_lock_file(&path) {
            Ok(Some(file)) => Ok(Self { _file: file }),
            Ok(None) => Err(config_error(
                format!("{OPERATION_IN_PROGRESS} {}", root.display()),
                [
                    format!(
                        "{} is held by another ambit command or the Ambit app",
                        path.display()
                    ),
                    "wait for it to finish, then run the command again".to_owned(),
                ],
            )),
            Err(error) => Err(cannot(io_message(&error, "open", &path))),
        }
    }
}

/// Where a project's operation lock lives.
pub fn operation_lock_path(root: &Path) -> PathBuf {
    join(&join(root, STATE_DIRNAME), OPERATION_LOCK_FILENAME)
}

/// Whether `error` is the one [`SetupLock::acquire`] returns for a lock another operation holds.
pub fn is_operation_in_progress(error: &AmbitError) -> bool {
    error.code == ExitCode::Config && error.message.starts_with(OPERATION_IN_PROGRESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tempdir;

    #[test]
    fn a_second_locker_is_refused_until_the_first_lets_go() {
        let dir = tempdir();
        let root = dir.path().to_path_buf();
        let held = SetupLock::acquire(&root).expect("the first lock");

        let contender = root.clone();
        let error = std::thread::spawn(move || SetupLock::acquire(&contender))
            .join()
            .expect("the thread finishes")
            .expect_err("a held lock");

        assert!(is_operation_in_progress(&error), "{error:?}");
        assert_eq!(error.code, ExitCode::Config);
        assert!(error.detail[1].contains("run the command again"));

        drop(held);

        SetupLock::acquire(&root).expect("the lock is free again");
        assert!(operation_lock_path(&root).is_file(), "never deleted");
    }

    #[test]
    fn refuses_a_root_that_is_not_there_without_creating_it() {
        let dir = tempdir();
        let root = dir.path().join("missing");
        let error = SetupLock::acquire(&root).expect_err("no root");

        assert_eq!(error.code, ExitCode::Config);
        assert!(!is_operation_in_progress(&error));
        assert!(!root.exists());
    }
}
