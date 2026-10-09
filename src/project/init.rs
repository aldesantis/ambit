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

pub const INIT_FILENAME: &str = CONFIG_FILENAMES[0];

pub const KEEP_FILENAME: &str = ".gitkeep";

pub const LOCAL_CATALOG: &str = "local";

const LOCAL_SOURCE: &str = "path:.";

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

pub fn scaffold_config() -> String {
    render_scaffold(&blocks())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScaffoldedFile {
    pub file: String,
    pub text: String,
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InitOptions {
    pub dry_run: bool,
    pub create_root: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitResult {
    pub created: Vec<ScaffoldedFile>,
    pub kept: Vec<String>,
    pub written: bool,
}

fn exists(target: &Path) -> bool {
    std::fs::metadata(target).is_ok()
}

fn is_directory(target: &Path) -> bool {
    std::fs::metadata(target).is_ok_and(|metadata| metadata.is_dir())
}

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

    if !options.create_root && !is_directory(project_dir) {
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
