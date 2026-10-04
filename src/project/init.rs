//! `ambit init`: scaffold a project, which is also a catalog.
//!
//! Every ambit project is technically a catalog: a project that ships a skill, a server, or a hook
//! of its own puts it in `skills/`, `mcps/`, or `hooks/` and lists itself under `catalogs:`, since
//! that's the only way to declare one. This command scaffolds both halves at once: `ambit.yml`, the
//! item directories, and a live `catalogs:` entry naming the project itself.
//!
//! Notes on a few decisions:
//!
//! - The item directories are always created, in every project, not behind a flag, so the
//!   scaffolded `local` entry is true rather than aspirational. They're created by writing
//!   `.gitkeep` files inside them, which also makes them survive the first commit (git tracks no
//!   empty directory).
//! - `local` is scaffolded live, but the `requires` entry selecting it is commented out. An entry
//!   matching nothing is exit 3, and a fresh project's `local` catalog is empty, so an active
//!   `local/*` requirement would fail `ambit validate` immediately.
//! - No CI workflow is scaffolded: a project is routinely an existing application, and writing into
//!   its `.github/workflows/` would be presumptuous. The workflow is a paste-able block in the
//!   README instead, running `ambit validate`.
//! - An existing `ambit.yml` is refused; an existing `.gitkeep` is reported as `kept`. The config is
//!   what makes the directory a project, so overwriting it would discard someone's work; a
//!   `.gitkeep` carries no bytes to lose.
//!
//! The scaffold's comments double as documentation of the entry grammar, since this is where a
//! person meets it while writing the `requires` list.
//!
//! The bytes are emitted, not templated: the file is a list of [`ScaffoldBlock`]s rendered by
//! [`render_scaffold`], so stripping the comments leaves exactly what ambit would emit from the
//! same values. The tests pin that equivalence rather than a golden copy of the prose.
//!
//! The commented-out `requires` example is emitted the same way and then prefixed, so it can't be
//! malformed YAML, and it quotes the alias the live `catalogs:` block declares, so uncommenting it
//! leaves a config that agrees with itself.

use std::path::Path;

use serde_json::json;

use crate::errors::{Result, config_error};
use crate::model::catalog::{HOOKS_DIRNAME, MCPS_DIRNAME, PACKS_DIRNAME, SKILLS_DIRNAME};
use crate::model::config::{
    CONFIG_FILENAMES, CONFIG_VERSION, DEFAULT_HARNESSES, existing_config_files,
};
use crate::model::scaffold::{ScaffoldBlock, render_scaffold};
use crate::util::cmp::js_cmp;
use crate::util::fs::{io_message, mkdir_p, write_text};
use crate::util::json::{JsonObject, JsonValue};
use crate::util::path::join;

#[cfg(test)]
mod tests;

/// The name `init` writes: the first of the two accepted config filenames.
pub const INIT_FILENAME: &str = CONFIG_FILENAMES[0];

/// What is written inside each item directory so it exists and survives a commit. Invisible to
/// catalog parsing, which reads only `skills/**`, `mcps/*.yml`, `hooks/**` and `packs/**`.
pub const KEEP_FILENAME: &str = ".gitkeep";

/// The alias the scaffolded `catalogs` entry gives the project's own directory.
pub const LOCAL_CATALOG: &str = "local";

/// The `source` that names the project itself: its own `skills/`, `mcps/` and `hooks/`.
const LOCAL_SOURCE: &str = "path:.";

/// The item directories the scaffold creates, in the order they are written.
const ITEM_DIRNAMES: &[&str] = &[HOOKS_DIRNAME, MCPS_DIRNAME, PACKS_DIRNAME, SKILLS_DIRNAME];

fn object(value: JsonValue) -> JsonObject {
    match value {
        JsonValue::Object(object) => object,
        _ => unreachable!("every scaffold value is built as an object literal"),
    }
}

fn lines(text: &[&str]) -> Vec<String> {
    text.iter().map(|&line| line.to_owned()).collect()
}

/// The scaffold, block by block.
///
/// Every block that carries a key sits in sorted-key order, and the commented `requires` example
/// sits where its key would go, so uncommenting it leaves the file sorted and matching what the
/// emitter produces from its values.
fn blocks() -> Vec<ScaffoldBlock> {
    vec![
        ScaffoldBlock {
            comment: lines(&[
                "ambit project config. `ambit install` reads this file, resolves a bundle of skills and",
                "MCP servers from it, and writes that bundle into each harness listed below.",
                "",
                "`version` is the only required key. Every definition lives in a file a catalog holds, so this",
                "project lists itself below: a skill of its own goes in `skills/<name>/SKILL.md`, a server in",
                "`mcps/<name>.yml`, a hook in `hooks/<name>/hook.yml`, a pack in `packs/<name>.yml`.",
            ]),
            ..ScaffoldBlock::default()
        },
        ScaffoldBlock {
            comment: lines(&[
                "The catalogs to draw packs, skills, MCP servers and hooks from.",
                "",
                "`local` is this project's own item directories, which `ambit init` created — a project that",
                "ships nothing simply leaves them empty, and an empty catalog costs nothing. Add another with a",
                "`source` of `owner/repo`, `owner/repo@ref`, a git URL, or `path:./relative/dir`, and a `ref`",
                "to pin it to: quote one that looks like a number — `ref: \"1234567\"` — or YAML will read it as",
                "one.",
                "",
                "The order carries no meaning: none takes precedence over another, and selecting one name from",
                "two of them is refused rather than settled here.",
                "",
                "A git catalog defaults to `trust: review`: `ambit install` refuses a hook or a stdio MCP server",
                "from it that `ambit.lock` does not hold yet, the first install included, until you re-run it",
                "with `--accept-exec`. Write `trust: full` on a catalog you trust to skip that. A `path:`",
                "catalog defaults to `full`.",
            ]),
            values: Some(object(json!({
                "catalogs": [{ "name": LOCAL_CATALOG, "source": LOCAL_SOURCE }],
            }))),
            ..ScaffoldBlock::default()
        },
        ScaffoldBlock {
            comment: lines(&[
                "The agent harnesses to install into: `claude`, `codex`, `copilot`, `cursor`, `devin`,",
                "`gemini`, `grok`, `kiro`, `opencode`.",
                "",
                "Skills go to `.agents/skills/` whichever are listed — one copy, however many tools read it.",
                "`claude` and `cursor` also get `.claude/skills` as a link to it, `kiro` gets `.kiro/skills`",
                "and `grok` gets `.grok/skills`, since none of them reads the shared directory natively.",
                "Each harness's MCP servers go in that harness's own config file: `.mcp.json`,",
                "`.codex/config.toml`, `.vscode/mcp.json`, `.cursor/mcp.json`, `.devin/mcp_config.json`,",
                "`.gemini/settings.json`, `.grok/config.toml`, `.kiro/settings/mcp.json`,",
                "`.opencode/opencode.jsonc`.",
            ]),
            values: Some(object(json!({ "harnesses": DEFAULT_HARNESSES }))),
            ..ScaffoldBlock::default()
        },
        ScaffoldBlock {
            comment: lines(&[
                "What this project selects — who this project is, in its catalogs' terms.",
                "",
                "Nothing is implicit: ambit adds nothing on its own, so an item no entry below reaches is not",
                "installed, however universal it looks. Each entry is one key naming what kind of thing it",
                "selects — `pack`, `skill`, `mcp` or `hook` — carrying the pattern to match names against. The",
                "kind is written out rather than guessed: a catalog's namespaces are flat and independent, so",
                "`mcp.sentry` is a plausible skill name and `sentry` a plausible server one namespace over.",
                "",
                "A `pack` is a capability whose only job is to pull in others, declared in the catalog with a",
                "name and a description — so `- pack: company/engineering` takes whatever that catalog says an",
                "engineer needs, skills, servers and hooks together, and `ambit search --capability pack \"*\"`",
                "says what that is.",
                "",
                "An address is `<catalog>/<pattern>`, where the catalog is an alias from `catalogs:` above.",
                "In a pattern, `*` matches any run of characters, including `.`, and a pattern without one is",
                "an exact name. `core.*` matches `core.a` and `core.a.b` but not `core` itself, so selecting a",
                "prefix and the item named exactly that takes two entries. `local/*` is a whole namespace.",
                "",
                "An entry that matches nothing is an error, not a silent miss — which is why this block is",
                "commented out: `local` is empty until this project ships something, and the entries below would",
                "fail on the project it was just scaffolded into. Uncomment them once there is something to take.",
            ]),
            example: Some(object(json!({
                "requires": [
                    { "pack": format!("{LOCAL_CATALOG}/*") },
                    { "skill": format!("{LOCAL_CATALOG}/*") },
                ],
            }))),
            ..ScaffoldBlock::default()
        },
        ScaffoldBlock {
            comment: lines(&[
                "The config format version. `1` is the only one this build understands.",
            ]),
            values: Some(object(json!({ "version": CONFIG_VERSION }))),
            ..ScaffoldBlock::default()
        },
    ]
}

/// The scaffolded `ambit.yml`, as bytes.
///
/// Pure and byte-stable: the output is a function of the blocks alone, so two runs on two machines
/// scaffold the same file.
pub fn scaffold_config() -> String {
    render_scaffold(&blocks())
}

/// One file the scaffold writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScaffoldedFile {
    /// Project-relative and `/`-separated: how output and messages name it.
    pub file: String,
    /// The bytes it holds. Empty for a `.gitkeep`, whose whole content is its path.
    pub text: String,
}

/// The scaffold: every file it writes, with its bytes, in path order.
///
/// Pure and byte-stable: nothing about the target directory reaches the contents, so two runs into
/// two differently named directories produce identical trees.
pub fn scaffold_project() -> Vec<ScaffoldedFile> {
    let mut files = vec![ScaffoldedFile {
        file: INIT_FILENAME.to_owned(),
        text: scaffold_config(),
    }];

    files.extend(ITEM_DIRNAMES.iter().map(|dirname| ScaffoldedFile {
        file: format!("{dirname}/{KEEP_FILENAME}"),
        text: String::new(),
    }));
    files.sort_by(|a, b| js_cmp(&a.file, &b.file));
    files
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
    /// The files written, or under `--dry-run` the ones that would be, in path order. Each carries
    /// its bytes, so a preview and a consuming tool both have what they need.
    pub created: Vec<ScaffoldedFile>,
    /// Scaffold files that were already there, left byte-identical, in path order.
    pub kept: Vec<String>,
    /// False under `--dry-run`, true otherwise. Carried explicitly because it's what distinguishes
    /// the preview from the real thing in `--json`, where there's no heading to say so.
    pub written: bool,
}

/// Whether something is at `target`, following symlinks as `stat` does.
fn exists(target: &Path) -> bool {
    std::fs::metadata(target).is_ok()
}

fn is_directory(target: &Path) -> bool {
    std::fs::metadata(target).is_ok_and(|metadata| metadata.is_dir())
}

/// Writes one scaffolded file, creating the directory that holds it.
///
/// `mkdir_p` is what creates the item directories: a `.gitkeep` and the directory it keeps arrive
/// together or not at all. It does not create the project root; [`init_project`] refuses a missing
/// one before reaching here.
///
/// # Errors
///
/// Exit 2 if the write fails, naming the file.
fn write(project_dir: &Path, scaffolded: &ScaffoldedFile) -> Result<()> {
    let target = join(project_dir, &scaffolded.file);
    let parent = target.parent().unwrap_or(project_dir);
    let fail = |message: String| {
        config_error(
            format!("cannot write {}", scaffolded.file),
            [
                message,
                format!("check that {} is writable", project_dir.display()),
            ],
        )
    };

    mkdir_p(parent).map_err(|error| fail(io_message(&error, parent)))?;
    write_text(&target, &scaffolded.text).map_err(|error| fail(io_message(&error, &target)))
}

/// Scaffolds a project in `project_dir`: `ambit.yml`, and the item directories that make it a
/// catalog of its own.
///
/// A directory that already holds either accepted config name is refused, under `--dry-run` too: a
/// preview of a command that would be refused is refused, the same stance `install --dry-run` takes
/// about ownership. Nothing is written on that path: not the config, and not a `.gitkeep`.
///
/// # Errors
///
/// Exit 2 if the directory already holds an ambit config, if it does not exist, or if a file cannot
/// be written.
pub fn init_project(project_dir: &Path, options: InitOptions) -> Result<InitResult> {
    let present = existing_config_files(project_dir)?;

    if !present.is_empty() {
        return Err(config_error(
            format!("refusing to overwrite {}", present.join(" and ")),
            [
                format!("{} already holds an ambit config", project_dir.display()),
                "edit it, or delete it and run `ambit init` again".to_owned(),
            ],
        ));
    }

    // A missing root is refused rather than created: `--project` naming the wrong path shouldn't
    // leave a project scaffolded in a directory nobody meant.
    if !is_directory(project_dir) {
        return Err(config_error(
            format!("cannot initialize {}", project_dir.display()),
            [
                "it is not a directory, and `init` creates no project root",
                "create it, or point `--project` at a directory that exists",
            ],
        ));
    }

    let mut created = Vec::new();
    let mut kept = Vec::new();

    for scaffolded in scaffold_project() {
        if exists(&join(project_dir, &scaffolded.file)) {
            kept.push(scaffolded.file);
        } else {
            created.push(scaffolded);
        }
    }

    if options.dry_run {
        return Ok(InitResult {
            created,
            kept,
            written: false,
        });
    }

    for scaffolded in &created {
        write(project_dir, scaffolded)?;
    }

    Ok(InitResult {
        created,
        kept,
        written: true,
    })
}
