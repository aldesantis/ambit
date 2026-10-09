use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::{AmbitError, ExitCode, Result, at, config_error};
use crate::model::config::{CatalogRef, ProjectConfig};
use crate::model::expectation::{Expectation, parse_expectations};
use crate::model::git::RefreshMode;
use crate::model::hook_entity::{
    HookEvent, HookType, command_program, parse_hook_entity, script_reference,
};
use crate::model::lock_file::read_catalog_pins;
use crate::model::mcp_entity::{McpTransport, parse_mcp_entity};
use crate::model::pack_entity::parse_pack_entity;
use crate::model::pattern::{Addressing, PatternEntry, parse_entries};
use crate::model::plugin::PluginMetadata;
use crate::model::requirement::CATALOG_SEPARATOR;
use crate::model::sources::{ResolvedSource, SourceContext, SourceRequest, resolve_source};
use crate::model::yaml::{YamlMapping, read_frontmatter_mapping, read_yaml_mapping};
use crate::util::cmp::js_cmp;
use crate::util::fs::{EntryKind, io_message, lstat_kind, read_dir_names};
use crate::util::path::join;
use crate::util::string_enum;
use crate::util::text::js_trim;

const REMOVED_REGISTRY_FILENAME: &str = "scopes.yml";

pub const SKILLS_DIRNAME: &str = "skills";
pub const MCPS_DIRNAME: &str = "mcps";

pub const PACKS_DIRNAME: &str = "packs";
pub const HOOKS_DIRNAME: &str = "hooks";

pub const SKILL_FILENAME: &str = "SKILL.md";

pub const HOOK_FILENAME: &str = "hook.yml";

const YAML_EXTENSIONS: &[&str] = &[".yml", ".yaml"];

pub const MCP_EXTENSIONS: &[&str] = YAML_EXTENSIONS;

pub const PACK_EXTENSIONS: &[&str] = YAML_EXTENSIONS;

pub const AMBIT_FRONTMATTER_KEY: &str = "ambit";

string_enum! {
    pub enum AnnotationKey {
        Requires => "requires",
        Expects => "expects",
    }
}

pub const ANNOTATION_KEYS: &[AnnotationKey] = AnnotationKey::ALL;

#[derive(Debug, Default)]
pub struct CatalogParseOptions<'a> {
    pub collect: Option<&'a mut Vec<AmbitError>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogSkill {
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub requires: Vec<PatternEntry>,
    pub expects: Vec<Expectation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPack {
    pub name: String,
    pub plugin: Option<PluginMetadata>,
    pub description: Option<String>,
    pub requires: Vec<PatternEntry>,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogMcp {
    pub name: String,
    pub transport: McpTransport,
    pub expects: Vec<Expectation>,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogHook {
    pub name: String,
    pub description: Option<String>,
    pub event: HookEvent,
    pub matcher: Option<String>,
    pub r#type: HookType,
    pub command: String,
    pub timeout: Option<i64>,
    pub expects: Vec<Expectation>,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    pub name: String,
    pub source: String,
    pub r#ref: Option<String>,
    pub root: PathBuf,
    pub commit: Option<String>,
    pub moving: Option<bool>,
    pub packs: Vec<CatalogPack>,
    pub skills: Vec<CatalogSkill>,
    pub mcps: Vec<CatalogMcp>,
    pub hooks: Vec<CatalogHook>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedPack {
    pub name: String,
    pub plugin: Option<PluginMetadata>,
    pub description: Option<String>,
    pub requires: Vec<PatternEntry>,
    pub catalog: String,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedSkill {
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub requires: Vec<PatternEntry>,
    pub expects: Vec<Expectation>,
    pub catalog: String,
    pub commit: Option<String>,
    // Machine-specific: never emit it on an output surface (golden files compare byte for byte).
    pub catalog_root: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedMcp {
    pub name: String,
    pub transport: McpTransport,
    pub expects: Vec<Expectation>,
    pub catalog: String,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedHook {
    pub name: String,
    pub description: Option<String>,
    pub event: HookEvent,
    pub matcher: Option<String>,
    pub r#type: HookType,
    pub command: String,
    pub timeout: Option<i64>,
    pub expects: Vec<Expectation>,
    pub catalog: String,
    pub path: String,
    pub commit: Option<String>,
    // Machine-specific: never emit it on an output surface.
    pub catalog_root: PathBuf,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergedCatalog {
    pub catalogs: Vec<String>,
    pub packs: Vec<MergedPack>,
    pub skills: Vec<MergedSkill>,
    pub mcps: Vec<MergedMcp>,
    pub hooks: Vec<MergedHook>,
}

#[derive(Debug, Default)]
pub struct CatalogLoadOptions<'a> {
    pub collect: Option<&'a mut Vec<AmbitError>>,
    pub refresh: Option<IndexMap<String, RefreshMode>>,
    pub pins: Option<IndexMap<String, String>>,
}

pub fn qualified_name(catalog: &str, name: &str) -> String {
    format!("{catalog}{CATALOG_SEPARATOR}{name}")
}

fn by_name<T>(mut items: Vec<T>, name: impl Fn(&T) -> &str) -> Vec<T> {
    items.sort_by(|a, b| js_cmp(name(a), name(b)));
    items
}

fn by_name_then_catalog<T>(mut items: Vec<T>, key: impl Fn(&T) -> (&str, &str)) -> Vec<T> {
    items.sort_by(|a, b| {
        let (a_name, a_catalog) = key(a);
        let (b_name, b_catalog) = key(b);

        js_cmp(a_name, b_name).then_with(|| js_cmp(a_catalog, b_catalog))
    });
    items
}

struct CatalogEntry {
    name: String,
    directory: bool,
}

struct CatalogFiles<'a> {
    root: &'a Path,
}

impl CatalogFiles<'_> {
    fn absolute(&self, relative: &str) -> PathBuf {
        join(self.root, relative)
    }

    fn is_file(&self, relative: &str) -> bool {
        std::fs::metadata(self.absolute(relative)).is_ok_and(|metadata| metadata.is_file())
    }

    fn is_directory(&self, relative: &str) -> bool {
        std::fs::metadata(self.absolute(relative)).is_ok_and(|metadata| metadata.is_dir())
    }

    fn entries(&self, relative: &str) -> Result<Vec<CatalogEntry>> {
        if !self.is_directory(relative) {
            return Ok(Vec::new());
        }

        let directory = self.absolute(relative);
        let unexpected =
            |error: std::io::Error, path: &Path| AmbitError::unexpected(io_message(&error, path));
        let mut names =
            read_dir_names(&directory).map_err(|error| unexpected(error, &directory))?;

        names.sort_by(|a, b| js_cmp(a, b));

        names
            .into_iter()
            .map(|name| {
                let path = join(&directory, &name);
                let kind = lstat_kind(&path).map_err(|error| unexpected(error, &path))?;

                Ok(CatalogEntry {
                    name,
                    directory: kind == EntryKind::Dir,
                })
            })
            .collect()
    }

    fn mapping(&self, relative: &str) -> Result<YamlMapping> {
        read_yaml_mapping(&self.absolute(relative), relative)
    }

    fn frontmatter(&self, relative: &str) -> Result<YamlMapping> {
        read_frontmatter_mapping(&self.absolute(relative), relative)
    }
}

fn under(parent: &str, relative: &str) -> String {
    if relative.is_empty() {
        parent.to_owned()
    } else {
        format!("{parent}/{relative}")
    }
}

pub fn resolve_catalog_root(
    catalog: &CatalogRef,
    context: &SourceContext,
    file: &str,
    refresh: Option<RefreshMode>,
    pin: Option<&str>,
) -> Result<ResolvedSource> {
    let resolved = resolve_source(
        &SourceRequest {
            source: catalog.source.clone(),
            r#ref: catalog.r#ref.clone(),
            subject: format!("catalog \"{}\"", catalog.name),
            r#where: at(file, None),
            refresh: Some(refresh.unwrap_or(RefreshMode::None)),
            pin: pin.map(str::to_owned),
        },
        context,
    )?;

    let Some(path) = &catalog.path else {
        return Ok(resolved);
    };

    Ok(ResolvedSource {
        root: resolve_catalog_path(catalog, &resolved.root, path, file)?,
        ..resolved
    })
}

fn resolve_catalog_path(
    catalog: &CatalogRef,
    source_root: &Path,
    path: &str,
    file: &str,
) -> Result<PathBuf> {
    let root = source_root.join(path);
    let inside = match (root.canonicalize(), source_root.canonicalize()) {
        (Ok(target), Ok(base)) => target.is_dir() && target.starts_with(base),
        _ => false,
    };

    if inside {
        return Ok(root);
    }

    Err(config_error(
        format!(
            "catalog \"{}\" has no directory at path \"{path}\" {}",
            catalog.name,
            at(file, None)
        ),
        [
            format!(
                "{} does not exist, is not a directory, or leads outside the source",
                root.display()
            ),
            "correct `path`, or drop it to read the source's root".to_owned(),
        ],
    ))
}

fn removed_registry() -> AmbitError {
    config_error(
        format!("the scope registry is gone ({REMOVED_REGISTRY_FILENAME})"),
        [
            format!(
                "scopes are gone; a group of items is a pack now — one `{PACKS_DIRNAME}/<name>.yml` requiring them, selected with `pack:`"
            ),
            format!(
                "delete {REMOVED_REGISTRY_FILENAME}, carrying each scope over as a pack requiring the items that declared it"
            ),
        ],
    )
}

pub fn skill_name_from_path(relative: &str) -> String {
    relative.replace('/', ".")
}

fn find_entity_directories(
    files: &CatalogFiles<'_>,
    parent: &str,
    marker: &str,
) -> Result<Vec<String>> {
    fn walk(
        files: &CatalogFiles<'_>,
        parent: &str,
        marker: &str,
        relative: &str,
        found: &mut Vec<String>,
    ) -> Result<()> {
        for entry in files.entries(&under(parent, relative))? {
            if entry.directory {
                walk(
                    files,
                    parent,
                    marker,
                    &under_relative(relative, &entry.name),
                    found,
                )?;
            } else if entry.name == marker {
                found.push(relative.to_owned());
            }
        }

        Ok(())
    }

    if !files.is_directory(parent) {
        return Ok(Vec::new());
    }

    let mut found = Vec::new();

    walk(files, parent, marker, "", &mut found)?;

    Ok(found)
}

fn under_relative(relative: &str, name: &str) -> String {
    if relative.is_empty() {
        name.to_owned()
    } else {
        format!("{relative}/{name}")
    }
}

fn skill_annotations(
    mapping: &YamlMapping,
) -> Result<(Option<String>, Vec<PatternEntry>, Vec<Expectation>)> {
    let description = mapping.optional_string("description")?;
    let Some(ambit) = mapping.optional_mapping(AMBIT_FRONTMATTER_KEY)? else {
        return Ok((description, Vec::new(), Vec::new()));
    };

    let keys: Vec<&str> = ANNOTATION_KEYS.iter().map(|key| key.as_str()).collect();

    ambit.reject_unknown_keys(&keys)?;

    Ok((
        description,
        parse_entries(&ambit, Addressing::Unqualified)?,
        parse_expectations(&ambit)?,
    ))
}

fn parse_skill(
    files: &CatalogFiles<'_>,
    relative: &str,
    catalog: &str,
    collect: &mut Option<&mut Vec<AmbitError>>,
) -> Result<CatalogSkill> {
    let file = format!("{SKILLS_DIRNAME}/{relative}/{SKILL_FILENAME}");

    if relative.is_empty() {
        return Err(config_error(
            format!("{SKILLS_DIRNAME}/{SKILL_FILENAME} is not inside a skill directory"),
            [
                "a skill's name is its path under `skills/`, so it needs at least one directory"
                    .to_owned(),
                format!("move it to {SKILLS_DIRNAME}/<name>/{SKILL_FILENAME}"),
            ],
        ));
    }

    let mapping = files.frontmatter(&file)?;
    let name = mapping.require_string("name")?;
    let derived = skill_name_from_path(relative);

    if name != derived
        && let Some(collected) = collect
    {
        let problem = mapping.key_error(
            "name",
            &format!("skill name \"{name}\" does not match its path"),
            vec![
                format!("{file} derives the name \"{derived}\""),
                "rename the directory, or correct `name` to match it".to_owned(),
            ],
        );

        collected.push(from_catalog(catalog, problem));
    }

    let (description, requires, expects) = skill_annotations(&mapping)?;

    Ok(CatalogSkill {
        name: derived,
        path: format!("{SKILLS_DIRNAME}/{relative}"),
        description,
        requires,
        expects,
    })
}

fn find_entity_files(
    files: &CatalogFiles<'_>,
    dirname: &str,
    nested: bool,
) -> Result<Vec<(String, String)>> {
    fn walk(
        files: &CatalogFiles<'_>,
        dirname: &str,
        nested: bool,
        relative: &str,
        by_name: &mut IndexMap<String, Vec<String>>,
    ) -> Result<()> {
        for entry in files.entries(&under(dirname, relative))? {
            let within = under_relative(relative, &entry.name);

            if entry.directory {
                if nested {
                    walk(files, dirname, nested, &within, by_name)?;
                }

                continue;
            }

            let Some(extension) = YAML_EXTENSIONS
                .iter()
                .find(|candidate| entry.name.ends_with(*candidate))
            else {
                continue;
            };

            let name = skill_name_from_path(&within[..within.len() - extension.len()]);

            by_name.entry(name).or_default().push(within);
        }

        Ok(())
    }

    if !files.is_directory(dirname) {
        return Ok(Vec::new());
    }

    // Keyed by derived name, so `a/b.yml` and `a.b.yml` collide and are refused.
    let mut by_name: IndexMap<String, Vec<String>> = IndexMap::new();

    walk(files, dirname, nested, "", &mut by_name)?;

    by_name
        .into_iter()
        .map(|(name, found)| {
            if found.len() > 1 {
                let paths: Vec<String> = found
                    .iter()
                    .map(|relative| format!("{dirname}/{relative}"))
                    .collect();

                return Err(config_error(
                    format!("{} both define \"{name}\"", paths.join(" and ")),
                    [
                        "ambit cannot tell which one is authoritative".to_owned(),
                        format!("delete one, keeping {dirname}/{name}{}", YAML_EXTENSIONS[0]),
                    ],
                ));
            }

            let file = format!("{dirname}/{}", found[0]);

            Ok((name, file))
        })
        .collect()
}

fn parse_pack_file(files: &CatalogFiles<'_>, name: &str, file: &str) -> Result<CatalogPack> {
    let mapping = files.mapping(file)?;
    let entity = parse_pack_entity(&mapping)?;

    if entity.name != name {
        return Err(mapping.key_error(
            "name",
            &format!("pack name \"{}\" does not match its path", entity.name),
            vec![
                format!("{file} derives the name \"{name}\""),
                format!(
                    "rename the file to {PACKS_DIRNAME}/{}{}, or correct `name`",
                    entity.name.replace('.', "/"),
                    PACK_EXTENSIONS[0]
                ),
            ],
        ));
    }

    Ok(CatalogPack {
        name: entity.name,
        plugin: entity.plugin,
        description: entity.description,
        requires: entity.requires,
        file: file.to_owned(),
    })
}

fn parse_mcp_file(files: &CatalogFiles<'_>, stem: &str, file: &str) -> Result<CatalogMcp> {
    let mapping = files.mapping(file)?;
    let entity = parse_mcp_entity(&mapping)?;

    if entity.name != stem {
        return Err(mapping.key_error(
            "name",
            &format!("MCP name \"{}\" does not match its filename", entity.name),
            vec![
                format!("{file} declares the name \"{stem}\""),
                format!(
                    "rename the file to {MCPS_DIRNAME}/{}{}, or correct `name`",
                    entity.name, MCP_EXTENSIONS[0]
                ),
            ],
        ));
    }

    Ok(CatalogMcp {
        name: entity.name,
        transport: entity.transport,
        expects: entity.expects,
        file: file.to_owned(),
    })
}

pub fn hook_command(hook: &MergedHook, root: &str) -> String {
    if hook.r#type != HookType::Script {
        return hook.command.clone();
    }

    let command = js_trim(&hook.command);
    let program = command_program(command);
    let script = format!("{root}/{}/{}", hook.name, script_reference(&program));

    format!("{script}{}", &command[program.len()..])
}

fn hook_directory_contents(files: &CatalogFiles<'_>, directory: &str) -> Result<Vec<String>> {
    fn walk(
        files: &CatalogFiles<'_>,
        directory: &str,
        relative: &str,
        found: &mut Vec<String>,
    ) -> Result<()> {
        for entry in files.entries(&under(directory, relative))? {
            let within = under_relative(relative, &entry.name);

            if entry.directory {
                walk(files, directory, &within, found)?;
            } else if within != HOOK_FILENAME {
                found.push(within);
            }
        }

        Ok(())
    }

    let mut found = Vec::new();

    walk(files, directory, "", &mut found)?;

    Ok(found)
}

fn assert_script_shipped(
    files: &CatalogFiles<'_>,
    mapping: &YamlMapping,
    directory: &str,
    name: &str,
    command: &str,
) -> Result<()> {
    let reference = script_reference(&command_program(command));

    if files.is_file(&format!("{directory}/{reference}")) {
        return Ok(());
    }

    let contents = hook_directory_contents(files, directory)?;

    Err(mapping.key_error(
        "command",
        &format!("hook \"{name}\" ships no {reference}"),
        vec![
            format!("`type: script` means `command` names a file {directory} holds"),
            if contents.is_empty() {
                format!("{directory} holds nothing but {HOOK_FILENAME}")
            } else {
                format!("{directory} holds: {}", contents.join(", "))
            },
            "correct the name, add the file to the hook's directory, or say `type: command` instead"
                .to_owned(),
        ],
    ))
}

fn parse_hook_directory(files: &CatalogFiles<'_>, relative: &str) -> Result<CatalogHook> {
    if relative.is_empty() {
        return Err(config_error(
            format!("{HOOKS_DIRNAME}/{HOOK_FILENAME} is not inside a hook directory"),
            [
                "a hook's name is its path under `hooks/`, so it needs at least one directory"
                    .to_owned(),
                format!("move it to {HOOKS_DIRNAME}/<name>/{HOOK_FILENAME}"),
            ],
        ));
    }

    let directory = format!("{HOOKS_DIRNAME}/{relative}");
    let file = format!("{directory}/{HOOK_FILENAME}");
    let mapping = files.mapping(&file)?;
    let entity = parse_hook_entity(&mapping)?;
    let derived = skill_name_from_path(relative);

    if entity.name != derived {
        return Err(mapping.key_error(
            "name",
            &format!("hook name \"{}\" does not match its path", entity.name),
            vec![
                format!("{file} derives the name \"{derived}\""),
                "rename the directory, or correct `name` to match it".to_owned(),
            ],
        ));
    }

    if entity.r#type == HookType::Script {
        assert_script_shipped(files, &mapping, &directory, &derived, &entity.command)?;
    }

    Ok(CatalogHook {
        name: entity.name,
        description: entity.description,
        event: entity.event,
        matcher: entity.matcher,
        r#type: entity.r#type,
        command: entity.command,
        timeout: entity.timeout,
        expects: entity.expects,
        path: directory,
    })
}

fn in_source(subject: &str, root: &Path, error: AmbitError) -> AmbitError {
    if error.code == ExitCode::Internal {
        return error;
    }

    let mut detail = vec![format!("in {subject} ({})", root.display())];

    detail.extend(error.detail);

    AmbitError::new(error.code, error.message, detail)
}

// Names the catalog, not its root: collected problems are compared byte for byte across machines.
fn from_catalog(name: &str, problem: AmbitError) -> AmbitError {
    let mut detail = vec![format!("in catalog \"{name}\"")];

    detail.extend(problem.detail);

    AmbitError::new(problem.code, problem.message, detail)
}

pub fn parse_catalog_directory(
    name: &str,
    source: &str,
    root: &Path,
    commit: Option<&str>,
    options: &mut CatalogParseOptions<'_>,
) -> Result<Catalog> {
    let files = CatalogFiles { root };

    let parsed = (|| -> Result<Catalog> {
        if files.is_file(REMOVED_REGISTRY_FILENAME) {
            return Err(removed_registry());
        }

        let mut packs = Vec::new();

        for (pack, file) in find_entity_files(&files, PACKS_DIRNAME, true)? {
            packs.push(parse_pack_file(&files, &pack, &file)?);
        }

        let mut skills = Vec::new();

        for relative in find_entity_directories(&files, SKILLS_DIRNAME, SKILL_FILENAME)? {
            skills.push(parse_skill(&files, &relative, name, &mut options.collect)?);
        }

        let mut mcps = Vec::new();

        for (stem, file) in find_entity_files(&files, MCPS_DIRNAME, false)? {
            mcps.push(parse_mcp_file(&files, &stem, &file)?);
        }

        let mut hooks = Vec::new();

        for relative in find_entity_directories(&files, HOOKS_DIRNAME, HOOK_FILENAME)? {
            hooks.push(parse_hook_directory(&files, &relative)?);
        }

        Ok(Catalog {
            name: name.to_owned(),
            source: source.to_owned(),
            r#ref: None,
            root: root.to_path_buf(),
            commit: commit.map(str::to_owned),
            moving: None,
            packs: by_name(packs, |item| &item.name),
            skills: by_name(skills, |item| &item.name),
            mcps: by_name(mcps, |item| &item.name),
            hooks: by_name(hooks, |item| &item.name),
        })
    })();

    parsed.map_err(|error| in_source(&format!("catalog \"{name}\""), root, error))
}

pub fn load_catalogs(
    config: &ProjectConfig,
    context: &SourceContext,
    options: &mut CatalogLoadOptions<'_>,
) -> Result<Vec<Catalog>> {
    let pins = match &options.pins {
        Some(pins) => pins.clone(),
        None => read_catalog_pins(&context.project_dir, config)?,
    };

    let mut catalogs = Vec::new();

    // Sequential: two catalogs can share one cache clone, which must not be fetched concurrently.
    for entry in &config.catalogs {
        let resolved = resolve_catalog_root(
            entry,
            context,
            &config.origin.file,
            options
                .refresh
                .as_ref()
                .and_then(|refresh| refresh.get(&entry.name).copied()),
            pins.get(&entry.name).map(String::as_str),
        )?;
        let mut parse_options = CatalogParseOptions {
            collect: options.collect.as_deref_mut(),
        };
        let parsed = parse_catalog_directory(
            &entry.name,
            &entry.source,
            &resolved.root,
            resolved.commit.as_deref(),
            &mut parse_options,
        )?;

        catalogs.push(Catalog {
            r#ref: entry.r#ref.clone(),
            moving: resolved.moving,
            ..parsed
        });
    }

    Ok(catalogs)
}

pub fn merge_catalogs(catalogs: &[Catalog]) -> MergedCatalog {
    let mut packs = Vec::new();
    let mut skills = Vec::new();
    let mut mcps = Vec::new();
    let mut hooks = Vec::new();

    for catalog in catalogs {
        for pack in &catalog.packs {
            packs.push(MergedPack {
                name: pack.name.clone(),
                plugin: pack.plugin.clone(),
                description: pack.description.clone(),
                requires: pack.requires.clone(),
                catalog: catalog.name.clone(),
                file: pack.file.clone(),
            });
        }

        for skill in &catalog.skills {
            skills.push(MergedSkill {
                name: skill.name.clone(),
                path: skill.path.clone(),
                description: skill.description.clone(),
                requires: skill.requires.clone(),
                expects: skill.expects.clone(),
                catalog: catalog.name.clone(),
                commit: catalog.commit.clone(),
                catalog_root: catalog.root.clone(),
            });
        }

        for mcp in &catalog.mcps {
            mcps.push(MergedMcp {
                name: mcp.name.clone(),
                transport: mcp.transport.clone(),
                expects: mcp.expects.clone(),
                catalog: catalog.name.clone(),
                file: mcp.file.clone(),
            });
        }

        for hook in &catalog.hooks {
            hooks.push(MergedHook {
                name: hook.name.clone(),
                description: hook.description.clone(),
                event: hook.event,
                matcher: hook.matcher.clone(),
                r#type: hook.r#type,
                command: hook.command.clone(),
                timeout: hook.timeout,
                expects: hook.expects.clone(),
                catalog: catalog.name.clone(),
                path: hook.path.clone(),
                commit: catalog.commit.clone(),
                catalog_root: catalog.root.clone(),
            });
        }
    }

    MergedCatalog {
        catalogs: catalogs
            .iter()
            .map(|catalog| catalog.name.clone())
            .collect(),
        packs: by_name_then_catalog(packs, |item| (&item.name, &item.catalog)),
        skills: by_name_then_catalog(skills, |item| (&item.name, &item.catalog)),
        mcps: by_name_then_catalog(mcps, |item| (&item.name, &item.catalog)),
        hooks: by_name_then_catalog(hooks, |item| (&item.name, &item.catalog)),
    }
}

#[cfg(test)]
mod tests;
