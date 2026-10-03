//! What ambit does to a project: install, prune, clean, status, doctor, update, init, and the
//! app's review and apply.

pub mod browse;
pub mod bundle_diff;
pub mod clean;
pub mod config_file;
pub mod doctor;
pub mod fingerprint;
pub mod gitignore;
pub mod init;
pub mod install;
pub mod lock;
pub mod operation_lock;
pub mod ownership;
pub mod prune;
pub mod review;
pub mod status;
pub mod update;
