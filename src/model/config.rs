//! `ambit.yml`: the project config.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::Result;
use crate::model::pattern::PatternEntry;

/// The only config version this build understands.
pub const CONFIG_VERSION: i64 = 1;

/// Used when `harnesses` is absent.
pub const DEFAULT_HARNESSES: &[&str] = &["claude"];

/// Accepted config filenames, in preference order. Having both is an error.
///
/// The first is the one `ambit init` writes.
pub const CONFIG_FILENAMES: [&str; 2] = ["ambit.yml", "ambit.yaml"];

/// A catalog to fetch and parse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogRef {
    pub name: String,
    pub source: String,
    /// Tag, branch, or commit. Absent means the source's default branch.
    pub r#ref: Option<String>,
}

/// Where the config came from, and where inside it the values live that a later stage judges.
///
/// Resolution runs long after parsing, so an error about a `requires` entry has no YAML node left
/// to point at, yet it still has to name the file and the line. This carries just enough of the
/// document's positions for that, keeping [`ProjectConfig`] itself plain data with no parser state
/// hanging off it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigOrigin {
    /// How the config file is named in messages: `ambit.yml` or `ambit.yaml`, project-relative.
    pub file: String,
    /// 1-based line each `requires` entry was written on, keyed by
    /// [`entry_yaml`](crate::model::pattern::entry_yaml).
    ///
    /// An entry renders to exactly one line, so the rendering is the key: two entries that render
    /// alike are the same selection, and the first line either was written on is the one a reader
    /// scanning downward finds.
    pub entry_lines: IndexMap<String, usize>,
}

/// A parsed, validated `ambit.yml`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectConfig {
    pub version: i64,
    /// Positions for the errors raised after parsing.
    pub origin: ConfigOrigin,
    pub harnesses: Vec<String>,
    /// Catalogs to fetch and parse, in the order they were listed.
    ///
    /// The order carries no meaning: every catalog's copy of a name survives the merge, so there is
    /// no precedence between them to establish. It is kept because it is what the config says, and
    /// because the lock lists catalogs as inputs.
    pub catalogs: Vec<CatalogRef>,
    /// What this project selects: pattern entries in the order they were written, literal
    /// duplicates dropped.
    ///
    /// Deduplicated here because an entry written twice is one selection and one finding. The
    /// order is the document's, so nothing downstream has to sort to be deterministic.
    pub requires: Vec<PatternEntry>,
}

/// A config file found in a project directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundConfig {
    /// Absolute path.
    pub path: PathBuf,
    /// The project-relative name to use in messages.
    pub file: String,
}

/// Parses an `ambit.yml` document. `file` is how it is named in error messages, conventionally
/// project-relative.
///
/// # Errors
///
/// Exit 2 for anything malformed.
pub fn parse_project_config(text: &str, file: &str) -> Result<ProjectConfig> {
    let _ = (text, file);
    todo!("port model/config.ts:parseProjectConfig")
}

/// Which accepted config filenames `project_dir` already holds, in preference order.
///
/// Shared with `ambit init`, whose question is the opposite of [`find_config_file`]'s: it must
/// refuse a directory that holds either name, naming the file it found rather than the one it was
/// about to write.
///
/// # Errors
///
/// Exit 2 when the directory cannot be inspected.
pub fn existing_config_files(project_dir: &Path) -> Result<Vec<String>> {
    let _ = project_dir;
    todo!("port model/config.ts:existingConfigFiles")
}

/// Finds the config file in `project_dir`.
///
/// # Errors
///
/// Exit 2 if there is no config, or more than one.
pub fn find_config_file(project_dir: &Path) -> Result<FoundConfig> {
    let _ = project_dir;
    todo!("port model/config.ts:findConfigFile")
}

/// Loads the config for a project directory.
///
/// # Errors
///
/// Exit 2 if the config is missing, ambiguous, or malformed.
pub fn load_project_config(project_dir: &Path) -> Result<ProjectConfig> {
    let _ = project_dir;
    todo!("port model/config.ts:loadProjectConfig")
}
