//! Catalogs: parsing one directory, loading every configured one, and merging them.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use crate::errors::{AmbitError, Result};
use crate::model::config::{CatalogRef, ProjectConfig};
use crate::model::expectation::Expectation;
use crate::model::git::RefreshMode;
use crate::model::hook_entity::{HookEvent, HookType};
use crate::model::mcp_entity::McpTransport;
use crate::model::pattern::PatternEntry;
use crate::model::plugin::PluginMetadata;
use crate::model::sources::{ResolvedSource, SourceContext};
use crate::util::string_enum;

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

/// The extensions a flat YAML document may carry, the one ambit writes first.
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
pub fn qualified_name(catalog: &str, name: &str) -> String {
    let _ = (catalog, name);
    todo!("port model/catalog.ts:qualifiedName")
}

/// Resolves a catalog's `source` to a directory on disk, fetching it if it is a git source.
///
/// `file` is how the config file is named in errors. `refresh` defaults to
/// [`RefreshMode::None`]; `pin` is the commit `ambit.lock` recorded for this catalog.
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
    let _ = (catalog, context, file, refresh, pin);
    todo!("port model/catalog.ts:resolveCatalogRoot")
}

/// The name/path convention: the path under `skills/`, `hooks/` or `packs/` with `/` read as `.`.
pub fn skill_name_from_path(relative: &str) -> String {
    let _ = relative;
    todo!("port model/catalog.ts:skillNameFromPath")
}

/// One hook's `command` as a harness should read it: a shipped script's path moved under `root`.
///
/// Only the program token is rewritten; everything after it is arguments. So `guard.sh --strict`
/// becomes `<root>/<name>/guard.sh --strict`. A `type: command` hook is returned verbatim.
pub fn hook_command(hook: &MergedHook, root: &str) -> String {
    let _ = (hook, root);
    todo!("port model/catalog.ts:hookCommand")
}

/// Parses the catalog rooted at `root`.
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
    let _ = (name, source, root, commit, options);
    todo!("port model/catalog.ts:parseCatalogDirectory")
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
    let _ = (config, context, options);
    todo!("port model/catalog.ts:loadCatalogs")
}

/// Merges catalogs into one namespace per kind, keeping every catalog's copy of every name.
///
/// Nothing is dropped and nothing is arbitrated. The collision is decided at materialization
/// instead: resolution refuses a selection holding two copies of one name (`assert_no_collisions`).
pub fn merge_catalogs(catalogs: &[Catalog]) -> MergedCatalog {
    let _ = catalogs;
    todo!("port model/catalog.ts:mergeCatalogs")
}
