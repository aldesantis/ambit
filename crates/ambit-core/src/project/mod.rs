//! What ambit does to a project: install, prune, clean, status, doctor, update, init.

// Reached by the desktop app's FFI layer, not by the CLI binary.
#[allow(dead_code)]
pub mod browse;
pub mod bundle_diff;
pub mod clean;
pub mod doctor;
pub mod gitignore;
pub mod init;
pub mod install;
pub mod lock;
pub mod ownership;
pub mod prune;
pub mod status;
pub mod update;
