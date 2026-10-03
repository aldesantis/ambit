//! `ambit init`: scaffold a project, which is also a catalog.

use std::path::Path;

use crate::errors::Result;
use crate::model::config::CONFIG_FILENAMES;

/// The name `init` writes: the first of the two accepted config filenames.
pub const INIT_FILENAME: &str = CONFIG_FILENAMES[0];

/// What is written inside each item directory so it exists and survives a commit. Invisible to
/// catalog parsing.
pub const KEEP_FILENAME: &str = ".gitkeep";

/// The alias the scaffolded `catalogs` entry gives the project's own directory.
pub const LOCAL_CATALOG: &str = "local";

/// The scaffolded `ambit.yml`, as bytes. Pure and byte-stable.
pub fn scaffold_config() -> String {
    todo!("port project/init.ts:scaffoldConfig")
}

/// One file the scaffold writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScaffoldedFile {
    /// Project-relative and `/`-separated: how output and messages name it.
    pub file: String,
    /// The bytes it holds. Empty for a `.gitkeep`, whose whole content is its path.
    pub text: String,
}

/// The scaffold: every file it writes, with its bytes, in path order. Pure and byte-stable.
pub fn scaffold_project() -> Vec<ScaffoldedFile> {
    todo!("port project/init.ts:scaffoldProject")
}

/// How an init was asked to behave.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InitOptions {
    /// `--dry-run`: report the files that would be written and touch nothing.
    pub dry_run: bool,
}

/// What an init produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitResult {
    /// The files written, or under `--dry-run` the ones that would be, in path order.
    pub created: Vec<ScaffoldedFile>,
    /// Scaffold files that were already there, left byte-identical, in path order.
    pub kept: Vec<String>,
    /// False under `--dry-run`, true otherwise. Carried explicitly because it's what distinguishes
    /// the preview from the real thing in `--json`.
    pub written: bool,
}

/// Scaffolds a project in `project_dir`: `ambit.yml`, and the item directories that make it a
/// catalog of its own.
///
/// A directory that already holds either accepted config name is refused, under `--dry-run` too.
///
/// # Errors
///
/// Exit 2 if the directory already holds an ambit config, if it does not exist, or if a file cannot
/// be written.
pub fn init_project(project_dir: &Path, options: InitOptions) -> Result<InitResult> {
    let _ = (project_dir, options);
    todo!("port project/init.ts:initProject")
}
