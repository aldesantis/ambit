//! The data ambit reads and records: configs, catalogs, entities, state, and the lock.

pub mod catalog;
pub mod config;
// The app's FFI layer calls this; the CLI binary does not.
#[allow(dead_code)]
pub mod config_edit;
pub mod documents;
pub mod expectation;
pub mod git;
pub mod hook_entity;
pub mod lock_file;
pub mod mcp_entity;
pub mod pack_entity;
pub mod pattern;
pub mod plugin;
pub mod reference;
pub mod requirement;
pub mod scaffold;
pub mod sources;
pub mod state;
pub mod yaml;
