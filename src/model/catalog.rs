//! Catalog parsing.
//!
//! A catalog is a plain skills repo: skills at `skills/<name>/SKILL.md`, MCP entities at
//! `mcps/<name>.yml`, hooks at `hooks/<name>/hook.yml`, and packs at `packs/<name>.yml` (which may
//! nest, so `packs/function/engineering.yml` is the pack `function.engineering`). Nothing here is
//! ambit-specific except one extra frontmatter key and these extra directories, both ignored by
//! other tools; that compatibility is a hard requirement.
//!
//! There is no catalog-side config: parsing scans the four directories and takes what is there. A
//! project's own config file at the catalog root is ignored rather than refused, because a project
//! that publishes its own items lists itself as `source: path:.`: a directory can be both a
//! catalog and a project at once.
//!
//! A skill's name is derived from its path, and the frontmatter `name` must agree; disagreement is
//! an error, because every other tool derives the name from the path and would install the skill
//! under a different name than ambit resolved.
//!
//! This module only reads from a directory. Turning a `source` (a local path, or a git repository
//! fetched into the cache) into a directory is `sources.rs`'s job, so parsing is identical
//! regardless of where the catalog came from.

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

/// The registry a catalog used to carry, kept only so its presence can be refused.
const REMOVED_REGISTRY_FILENAME: &str = "scopes.yml";

pub const SKILLS_DIRNAME: &str = "skills";
pub const MCPS_DIRNAME: &str = "mcps";

/// Where packs live within a catalog.
///
/// A pack is a file, not a directory, like an MCP entity and unlike a skill or a hook: it ships no
/// bytes. Its name is its path under `packs/` with the extension dropped and `/` read as `.`, the
/// same convention a skill's name follows, so `packs/function/engineering.yml` and
/// `packs/function.engineering.yml` both declare `function.engineering`. Declaring the same name
/// both ways is refused.
pub const PACKS_DIRNAME: &str = "packs";
pub const HOOKS_DIRNAME: &str = "hooks";

/// The file whose presence makes a directory a skill.
///
/// Uppercase because harnesses and other tools already walk `skills/<name>/SKILL.md`; ambit
/// matches that spelling rather than choosing its own.
pub const SKILL_FILENAME: &str = "SKILL.md";

/// The file whose presence makes a directory a hook.
///
/// A hook is always a directory, like a skill and unlike an MCP entity, because a hook may ship its
/// own script; one that does not is just a directory holding this one file.
///
/// Lowercase, unlike [`SKILL_FILENAME`], because nothing outside ambit reads it, so it is spelled
/// like every other file ambit owns.
pub const HOOK_FILENAME: &str = "hook.yml";

/// The extensions a flat-file item may carry, in preference order. One stem carrying both is an
/// error. Both are read because either is valid; `.yml` is the one ambit names in a refusal.
const YAML_EXTENSIONS: &[&str] = &[".yml", ".yaml"];

/// MCP entity extensions; see `YAML_EXTENSIONS`.
pub const MCP_EXTENSIONS: &[&str] = YAML_EXTENSIONS;

/// Pack extensions: the same two, a pack being a flat document as an MCP entity is.
pub const PACK_EXTENSIONS: &[&str] = YAML_EXTENSIONS;

/// The one top-level `SKILL.md` frontmatter key ambit owns.
///
/// Every annotation lives under it, so ambit's keys can never collide with a key a harness defines
/// at the top level, however either grows.
pub const AMBIT_FRONTMATTER_KEY: &str = "ambit";

string_enum! {
    /// The keys ambit reads under [`AMBIT_FRONTMATTER_KEY`], in the order the format tabulates them.
    pub enum AnnotationKey {
        Requires => "requires",
        Expects => "expects",
    }
}

/// Every annotation key, in declaration order.
pub const ANNOTATION_KEYS: &[AnnotationKey] = AnnotationKey::ALL;

/// How a catalog is parsed when the caller wants every problem rather than only the first.
///
/// Only validation passes a collector. Everything else parses strictly, because a resolution that
/// carried on past a broken skill would install something nobody described.
#[derive(Debug, Default)]
pub struct CatalogParseOptions<'a> {
    /// Receives a problem that would otherwise have been returned, letting parsing continue past
    /// it.
    ///
    /// Exactly one problem takes this route: a skill whose frontmatter `name` disagrees with its
    /// path. It is the one violation parsing can recover from, by taking the path's answer, because
    /// every other tool derives the name from the path anyway.
    pub collect: Option<&'a mut Vec<AmbitError>>,
}

/// A skill as the catalog declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogSkill {
    /// Derived from `path`, and equal to the frontmatter `name`.
    pub name: String,
    /// The skill directory, relative to the catalog root, `/`-separated.
    pub path: String,
    /// The harness's own summary, carried through to every report that lists the skill.
    pub description: Option<String>,
    /// What this skill pulls into a bundle with it: a `requires` list in the same entry grammar a
    /// project selects with, minus the qualifier. In the order the author wrote them.
    ///
    /// Unqualified, and confined to this catalog: the alias belongs to the consumer's config, so a
    /// catalog author cannot write one, and a catalog can only require what it ships.
    pub requires: Vec<PatternEntry>,
    /// What must be true of the world for this skill to work, each entry naming its own kind. In
    /// the order the author wrote them.
    pub expects: Vec<Expectation>,
}

/// A pack as one catalog declares it, carrying the document it was read from.
///
/// The fields of [`PackEntity`](crate::model::pack_entity::PackEntity), plus `file`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPack {
    pub name: String,
    pub plugin: Option<PluginMetadata>,
    pub description: Option<String>,
    pub requires: Vec<PatternEntry>,
    /// The file that defines it, relative to the catalog root, whichever extension it carries.
    pub file: String,
}

/// An MCP entity as one catalog declares it, carrying the document it was read from.
///
/// The fields of [`McpEntity`](crate::model::mcp_entity::McpEntity), plus `file`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogMcp {
    pub name: String,
    pub transport: McpTransport,
    pub expects: Vec<Expectation>,
    /// The file that defines it, relative to the catalog root, whichever extension it actually
    /// carries.
    ///
    /// Carried from parsing rather than derived from the name, because `mcps/<name>.yml` is only
    /// the extension ambit writes; an entity spelled `.yaml` has no `.yml` to fall back to.
    pub file: String,
}

/// A hook as one catalog declares it, carrying the directory it was read from.
///
/// The fields of [`HookEntity`](crate::model::hook_entity::HookEntity), plus `path`.
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
    /// The hook directory, relative to the catalog root, `/`-separated.
    pub path: String,
}

/// One parsed catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Catalog {
    pub name: String,
    /// The `source` it was resolved from, as written in config.
    pub source: String,
    /// The `ref` its config entry asked for, as written. Absent when the entry named none, which
    /// means the source's default branch. Carried alongside `commit` because the lock records both:
    /// the commit says what was installed, the ref says what will be resolved next time.
    pub r#ref: Option<String>,
    /// Absolute path to the catalog root on disk.
    pub root: PathBuf,
    /// The commit its contents are, for a git source. Absent for a `path:` source, which has no
    /// revision: a working directory is whatever it currently says.
    pub commit: Option<String>,
    /// Whether its `ref` is one that can move, as against a `ref` naming a commit.
    ///
    /// Present only when the load refreshed this catalog: it is `ambit outdated`'s question, and
    /// only a run that reached the remote can answer it without guessing.
    pub moving: Option<bool>,
    /// Packs, sorted by name.
    pub packs: Vec<CatalogPack>,
    /// Skills, sorted by name.
    pub skills: Vec<CatalogSkill>,
    /// MCP entities, sorted by name.
    pub mcps: Vec<CatalogMcp>,
    /// Hooks, sorted by name.
    pub hooks: Vec<CatalogHook>,
}

/// A pack in the merged view, tagged with the catalog it came from.
///
/// No `catalog_root` and no `commit`: a pack ships no bytes, so there is nothing to materialize out
/// of a catalog directory and no revision to pin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedPack {
    pub name: String,
    pub plugin: Option<PluginMetadata>,
    pub description: Option<String>,
    pub requires: Vec<PatternEntry>,
    pub catalog: String,
    /// The file that defines it inside that catalog, catalog-relative.
    pub file: String,
}

/// A skill in the merged view, tagged with the catalog it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedSkill {
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub requires: Vec<PatternEntry>,
    pub expects: Vec<Expectation>,
    pub catalog: String,
    /// The commit the skill's bytes came from, inherited from its catalog. Absent for a `path:`
    /// source, which has no revision.
    pub commit: Option<String>,
    /// Absolute path to that catalog's root on disk, so materialization can find the skill without
    /// looking the catalog up again. Deliberately absent from every output surface: it is
    /// machine-specific, and golden files must not carry it.
    pub catalog_root: PathBuf,
}

/// An MCP entity in the merged view, tagged with the catalog it came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergedMcp {
    pub name: String,
    pub transport: McpTransport,
    pub expects: Vec<Expectation>,
    pub catalog: String,
    /// The file that defines it inside that catalog, catalog-relative. Always present: every
    /// definition lives in a file.
    pub file: String,
}

/// A hook in the merged view, tagged with the catalog it came from.
///
/// The union of what [`MergedSkill`] needs and what [`MergedMcp`] needs, because a hook is both
/// kinds of thing at once: it renders into a harness's config file, and, when it ships a script, it
/// also materializes a directory.
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
    /// The hook directory inside that catalog, catalog-relative.
    pub path: String,
    /// The commit the hook's bytes came from, when its catalog has one.
    pub commit: Option<String>,
    /// Absolute path to that catalog's root on disk. Deliberately absent from every output
    /// surface: it is machine-specific.
    pub catalog_root: PathBuf,
}

/// Every configured catalog, merged into one namespace per kind: every catalog's copy of every
/// name.
///
/// A name is not an identity here. Two catalogs may both provide `house-style`, and both copies
/// survive the merge, each identified by its catalog and its name (see [`qualified_name`]). A
/// lookup by name alone answers with a set rather than an item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergedCatalog {
    /// Catalog names, in config order. A record of what the config listed, nothing more.
    pub catalogs: Vec<String>,
    pub packs: Vec<MergedPack>,
    pub skills: Vec<MergedSkill>,
    pub mcps: Vec<MergedMcp>,
    pub hooks: Vec<MergedHook>,
}

/// How a load reaches each catalog's source, on top of what parsing needs.
#[derive(Debug, Default)]
pub struct CatalogLoadOptions<'a> {
    /// See [`CatalogParseOptions::collect`].
    pub collect: Option<&'a mut Vec<AmbitError>>,
    /// How each catalog may consult its remote, keyed by catalog name.
    ///
    /// A name the map does not hold, and an absent map, resolves from the cache exactly as every
    /// other command does. Per catalog rather than per run because `ambit update company` moves only
    /// one pin.
    pub refresh: Option<IndexMap<String, RefreshMode>>,
    /// The commit an earlier resolution recorded for each catalog, keyed by catalog name:
    /// `ambit.lock`'s pins, as [`read_catalog_pins`](crate::model::lock_file::read_catalog_pins)
    /// reads them.
    ///
    /// Omitting this reads the project's lock; it does not skip pinning. A caller opts out by
    /// passing an explicit map, an empty one to pin nothing. Only `install` and `update` do that.
    ///
    /// A `refresh` for the same catalog wins over its pin: the two refreshing commands were asked
    /// for a newer commit than the pin holds.
    pub pins: Option<IndexMap<String, String>>,
}

/// The address of one item in the merged catalog: `<catalog>/<name>`.
///
/// Used anywhere a single string per item is needed (a map key, a JSON record key, a visited set),
/// so two catalogs' copies of `house-style` cannot collapse into one entry by accident.
///
/// A *bundle* is the one view where a bare name is still an identity, because resolution refuses a
/// selection holding two copies of a name (`assert_no_collisions`), not because the merge
/// guarantees it.
pub fn qualified_name(catalog: &str, name: &str) -> String {
    format!("{catalog}{CATALOG_SEPARATOR}{name}")
}

/// One catalog's items in name order, which is a total order within a single catalog.
fn by_name<T>(mut items: Vec<T>, name: impl Fn(&T) -> &str) -> Vec<T> {
    items.sort_by(|a, b| js_cmp(name(a), name(b)));
    items
}

/// Merged items in name order, then catalog order.
///
/// Name first, so two catalogs' copies of one name sit next to each other in listings like
/// `ambit search`. Catalog second, because a name alone is no longer a total order across catalogs,
/// and falling back to config order would make listings depend on it again.
fn by_name_then_catalog<T>(mut items: Vec<T>, key: impl Fn(&T) -> (&str, &str)) -> Vec<T> {
    items.sort_by(|a, b| {
        let (a_name, a_catalog) = key(a);
        let (b_name, b_catalog) = key(b);

        js_cmp(a_name, b_name).then_with(|| js_cmp(a_catalog, b_catalog))
    });
    items
}

/// One directory entry, and whether it is itself a directory (a symlink is not, as a `Dirent`
/// reports it).
struct CatalogEntry {
    name: String,
    directory: bool,
}

/// A catalog's files, addressed the way its own errors and reports address them.
///
/// Every read parsing does goes through here, so a catalog-relative `/`-separated path is the only
/// kind of path the walk below deals in: the root is joined on in one place, keeping absolute
/// paths out of anything that reports a file.
struct CatalogFiles<'a> {
    root: &'a Path,
}

impl CatalogFiles<'_> {
    fn absolute(&self, relative: &str) -> PathBuf {
        join(self.root, relative)
    }

    /// Whether `relative` is a regular file, following symlinks. Any failure counts as no.
    fn is_file(&self, relative: &str) -> bool {
        std::fs::metadata(self.absolute(relative)).is_ok_and(|metadata| metadata.is_file())
    }

    /// Whether `relative` is a directory, following symlinks. Any failure counts as no.
    fn is_directory(&self, relative: &str) -> bool {
        std::fs::metadata(self.absolute(relative)).is_ok_and(|metadata| metadata.is_dir())
    }

    /// The entries of a directory, in name order, so a catalog parses identically whatever the
    /// filesystem says, or none at all when it is not there.
    ///
    /// # Errors
    ///
    /// Exit 1 when a directory that exists cannot be listed.
    fn entries(&self, relative: &str) -> Result<Vec<CatalogEntry>> {
        if !self.is_directory(relative) {
            return Ok(Vec::new());
        }

        let directory = self.absolute(relative);
        let unexpected = |error: std::io::Error, path: &Path| {
            AmbitError::unexpected(io_message(&error, "scandir", path))
        };
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

    /// Parses a YAML file under the loader's rules.
    fn mapping(&self, relative: &str) -> Result<YamlMapping> {
        read_yaml_mapping(&self.absolute(relative), relative)
    }

    /// Parses a Markdown file's frontmatter block under the loader's rules.
    fn frontmatter(&self, relative: &str) -> Result<YamlMapping> {
        read_frontmatter_mapping(&self.absolute(relative), relative)
    }
}

/// `parent/relative`, or `parent` alone for the walk's starting point.
fn under(parent: &str, relative: &str) -> String {
    if relative.is_empty() {
        parent.to_owned()
    } else {
        format!("{parent}/{relative}")
    }
}

/// Resolves a catalog's `source` to a directory on disk, fetching it if it is a git source.
///
/// `file` is how the config file is named in errors. Catalog entries carry no line of their own,
/// so the message names the file alone. `refresh` is how much of the remote this one catalog may
/// consult; absent means [`RefreshMode::None`], which is every command but `ambit outdated` and
/// `ambit update`. `pin` is the commit an earlier resolution recorded for this catalog, from
/// `ambit.lock`: the catalog resolves to that commit rather than to whatever its `ref` names now
/// (see [`SourceRequest::pin`]).
///
/// # Errors
///
/// Exit 2 for a source ambit cannot read, a missing directory, or an unknown ref; exit 4 if a fetch
/// fails.
pub fn resolve_catalog_root(
    catalog: &CatalogRef,
    context: &SourceContext,
    file: &str,
    refresh: Option<RefreshMode>,
    pin: Option<&str>,
) -> Result<ResolvedSource> {
    resolve_source(
        &SourceRequest {
            source: catalog.source.clone(),
            r#ref: catalog.r#ref.clone(),
            subject: format!("catalog \"{}\"", catalog.name),
            r#where: at(file, None),
            refresh: Some(refresh.unwrap_or(RefreshMode::None)),
            pin: pin.map(str::to_owned),
        },
        context,
    )
}

/// The refusal for a catalog that still holds the scope registry.
///
/// A hard break, and loud on purpose: the file still parses as YAML and every scope in it still
/// looks active, so silence would leave an author believing the catalog is labelled when nothing
/// in it is. The message carries the whole rewrite, since it is short enough to state.
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

/// The name/path convention: the path under `skills/`, `hooks/` or `packs/` with `/` read as `.`.
///
/// One function for all three, since it is one convention: a hook is named from its directory
/// exactly as a skill is, and a pack from its file with the extension already dropped.
pub fn skill_name_from_path(relative: &str) -> String {
    relative.replace('/', ".")
}

/// Every directory under `parent` holding `marker`, relative to `parent` and `/`-separated.
///
/// The walk skills and hooks share: both are named from their path under one directory, and both
/// are found by the file that marks one.
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

/// `relative/name`, or `name` alone at the top of a walk.
fn under_relative(relative: &str, name: &str) -> String {
    if relative.is_empty() {
        name.to_owned()
    } else {
        format!("{relative}/{name}")
    }
}

/// What ambit reads off a skill's frontmatter once its name is settled: the description, the
/// `requires` list, and the `expects` list.
///
/// Unknown keys are allowed at the top level, since that block is the harness's and ambit is a
/// guest in it. Unknown keys under `ambit:` are rejected, since that block is ambit's own: a
/// misspelled `require:` there would otherwise silently declare nothing.
///
/// # Errors
///
/// Exit 2 for an `ambit:` that is not a mapping, or a key under it the format does not define.
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
        // Unqualified: a catalog author cannot write a consumer's alias, so the entry resolves
        // within this catalog.
        parse_entries(&ambit, Addressing::Unqualified)?,
        parse_expectations(&ambit)?,
    ))
}

/// Parses one skill directory.
///
/// With a `collect`or, a name that disagrees with its path is reported through it and the path's
/// name is used, rather than returned as the error; see [`CatalogParseOptions`].
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

    if name != derived {
        let problem = mapping.key_error(
            "name",
            &format!("skill name \"{name}\" does not match its path"),
            vec![
                format!("{file} derives the name \"{derived}\""),
                "rename the directory, or correct `name` to match it".to_owned(),
            ],
        );

        match collect {
            None => return Err(problem),
            Some(collected) => collected.push(from_catalog(catalog, problem)),
        }
    }

    let (description, requires, expects) = skill_annotations(&mapping)?;

    // The path's name, always: it is what every other tool would install the skill under, so a
    // collected disagreement does not cascade into a second, invented problem.
    Ok(CatalogSkill {
        name: derived,
        path: format!("{SKILLS_DIRNAME}/{relative}"),
        description,
        requires,
        expects,
    })
}

/// The flat-file item names under `dirname`, each with the one file that defines it.
///
/// One walk for both namespaces that are documents rather than directories: `mcps/` and `packs/`.
///
/// `nested` says whether subdirectories are walked, and their segments joined into the name the way
/// [`skill_name_from_path`] joins a skill's. `mcps/` is flat: a server is one file directly under
/// it, and reading a nested one would give an existing catalog names it never declared. `packs/` is
/// nested, since a catalog offering many packs needs a way to group them.
///
/// # Errors
///
/// Exit 2 when two files define one name.
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

    // Keyed by the derived name rather than by the filename, so the two spellings that can produce
    // one name (`a/b.yml` and `a.b.yml`) collide here and are refused together with the two
    // extensions.
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

/// Parses one pack document: reads it, and checks that what it calls itself matches its filename.
///
/// A pack has no directory and no bytes, so this is the whole of loading one.
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

/// One hook's `command` as a harness should read it: a shipped script's path moved under `root`.
///
/// A catalog declares `command: guard.sh`, naming a file relative to the hook's own directory: a
/// location that exists in the catalog but not where a harness looks. Once installed the script
/// sits at `<root>/<name>/guard.sh`; `root` is how each harness spells the way there (its
/// profile's `hook_config` in `harness/definitions.rs`).
///
/// Only the program token is rewritten. Everything after it is arguments, and a `command` is a
/// shell fragment ambit does not parse; rewriting inside it could corrupt a quoted string or an
/// unrelated path. So `guard.sh --strict` becomes `<root>/<name>/guard.sh --strict`.
///
/// A `type: command` hook is returned verbatim: `npx --yes prettier` is a command line the harness
/// runs as-is, and prefixing it with a directory would break it.
pub fn hook_command(hook: &MergedHook, root: &str) -> String {
    if hook.r#type != HookType::Script {
        return hook.command.clone();
    }

    let command = js_trim(&hook.command);
    let program = command_program(command);
    let script = format!("{root}/{}/{}", hook.name, script_reference(&program));

    format!("{script}{}", &command[program.len()..])
}

/// Every file a hook's directory holds besides its own `hook.yml`, in path order.
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

/// Asserts that a `type: script` hook ships the file its `command` names.
///
/// A hook that claims a file it does not hold is refused, naming the directory's contents: without
/// this check, installing it would write a command pointing at bytes that never arrive.
///
/// A `type: command` hook is never checked: its `command` is a command line, and whether a file of
/// that name happens to sit in the directory is a coincidence.
///
/// `name` is the hook's name as its path derives it, which is what it will be installed under.
///
/// # Errors
///
/// Exit 2 for a `command` that names a file the directory does not hold.
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

/// Parses one hook directory.
///
/// A disagreement between `name` and the path is returned rather than collected, unlike a skill's:
/// the recovery there exists because every other tool installs a skill under its path's name, and
/// nothing but ambit reads a `hooks/` directory at all.
///
/// # Errors
///
/// Exit 2 for a malformed document, a `name` that disagrees with the path, or a `command` naming a
/// file the directory does not hold.
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

/// Adds where an error came from, so a message about `skills/a/b/SKILL.md` says which of several
/// sources holds that path. Prepended, keeping the concrete next step last.
///
/// An internal error (exit 1) is left alone: it is not about the catalog's contents.
fn in_source(subject: &str, root: &Path, error: AmbitError) -> AmbitError {
    if error.code == ExitCode::Internal {
        return error;
    }

    let mut detail = vec![format!("in {subject} ({})", root.display())];

    detail.extend(error.detail);

    AmbitError::new(error.code, error.message, detail)
}

/// The same attribution for a *collected* problem, naming the catalog but not its root.
///
/// The root is a machine path (a cache checkout, for a git source) and a collected problem is
/// printed as part of a report that output tests compare byte for byte across machines. The
/// catalog's name is enough to disambiguate two catalogs holding the same relative path.
fn from_catalog(name: &str, problem: AmbitError) -> AmbitError {
    let mut detail = vec![format!("in catalog \"{name}\"")];

    detail.extend(problem.detail);

    AmbitError::new(problem.code, problem.message, detail)
}

/// Parses the catalog rooted at `root`.
///
/// `name` is the catalog's name, as errors report it; `source` is the `source` it was resolved
/// from; `commit` is the commit the directory holds, for a git source. `options` carries a
/// collector for the one problem parsing can continue past; see [`CatalogParseOptions`].
///
/// # Errors
///
/// Exit 2 for a leftover scope registry, a malformed file, or a name that disagrees with its path.
pub fn parse_catalog_directory(
    name: &str,
    source: &str,
    root: &Path,
    commit: Option<&str>,
    options: &mut CatalogParseOptions<'_>,
) -> Result<Catalog> {
    let files = CatalogFiles { root };

    let parsed = (|| -> Result<Catalog> {
        // The one file at a catalog root ambit still has an opinion about: it must not be there.
        // Nothing else is read here: a directory holding none of the four subdirectories is a
        // catalog with zero items, which the patterns selecting from it report better than a
        // missing-file error here could.
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

/// Loads every catalog the config declares, in config order.
///
/// Sequential: two catalogs can be two refs of one repository, and a shared cache directory must
/// not be fetched into by two operations at once.
///
/// # Errors
///
/// Exit 2 for an unresolvable source or a malformed catalog; exit 4 if a fetch fails.
pub fn load_catalogs(
    config: &ProjectConfig,
    context: &SourceContext,
    options: &mut CatalogLoadOptions<'_>,
) -> Result<Vec<Catalog>> {
    // Read here rather than at each call site, so no command can forget and resolve a different
    // commit than the install it is describing. See `CatalogLoadOptions::pins` for how a caller
    // opts out.
    let pins = match &options.pins {
        Some(pins) => pins.clone(),
        None => read_catalog_pins(&context.project_dir, config)?,
    };

    let mut catalogs = Vec::new();

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

        // `ref` and `moving` are facts about the config entry and how its source answered, not
        // about the directory that was parsed, so both are attached here rather than threaded
        // through parsing, which also keeps a `path:` catalog from having to invent either.
        catalogs.push(Catalog {
            r#ref: entry.r#ref.clone(),
            moving: resolved.moving,
            ..parsed
        });
    }

    Ok(catalogs)
}

/// Merges catalogs into one namespace per kind, keeping every catalog's copy of every name.
///
/// Nothing is dropped and nothing is arbitrated. Two catalogs both providing `house-style` is a
/// non-event here: both copies are in the merged view, each addressable by its catalog and its
/// name, so `catalogs:` order settles nothing.
///
/// The collision is decided at materialization instead. Harness layout is flat and externally
/// imposed (Claude reads `.claude/skills/<name>`), so two copies of one name that are both
/// *selected* would want one path, and resolution refuses that (`assert_no_collisions`). Refusing a
/// selection is different from refusing a catalog: a name two catalogs ship costs nothing until a
/// project asks for both.
///
/// Every item in the result came out of a catalog directory, since that is the only place a
/// definition can be written.
pub fn merge_catalogs(catalogs: &[Catalog]) -> MergedCatalog {
    let mut packs = Vec::new();
    let mut skills = Vec::new();
    let mut mcps = Vec::new();
    let mut hooks = Vec::new();

    for catalog in catalogs {
        for pack in &catalog.packs {
            // Neither `commit` nor `catalog_root`: a pack ships no bytes, so there is nothing to
            // pin and nothing to materialize out of the directory it was read from.
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
            // `catalog_root` for the same reason a skill carries one: a hook that ships a script is
            // materialized out of the catalog it came from.
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
