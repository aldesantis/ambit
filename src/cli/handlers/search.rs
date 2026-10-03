//! `ambit search <pattern>`: search the merged catalog.
//!
//! The subject is the project's merged view: several catalogs plus one `ambit.yml`, which no single
//! catalog directory holds on its own. `ambit search "*"` prints the whole merged catalog. `--json`
//! output carries no absolute paths and emits every record in the merged catalog's own order (by
//! name, then by catalog), so it is comparable between machines and stable enough to commit as a
//! golden file.
//!
//! Each JSON record is keyed by an item's address, `<catalog>/<name>`, not by its name alone: a
//! name is not unique in this view (two catalogs may both provide `house-style`), and a name-keyed
//! record would drop one copy. The text form doesn't need this since it already prints one row per
//! copy with the catalog in its own column.
//!
//! Three filters: `<pattern>` matches names, `--capability` picks namespaces, `--catalog` picks
//! catalogs. Repeating one flag widens it (`--catalog a --catalog b` is *either*); different flags
//! narrow (a skill in neither catalog is not a result).
//!
//! A pattern matching nothing is exit 0. This differs from a `requires` entry matching nothing: a
//! requirement that reaches nothing is a broken config, while a search that finds nothing is just
//! the answer to the search.

use indexmap::IndexSet;

use crate::cli::commands::{CommandContext, json_requested, list_of, source_context_of};
use crate::cli::output::{keyed, print_sections, section};
use crate::errors::{ExitCode, Result, config_error};
use crate::model::catalog::{
    Catalog, CatalogLoadOptions, MergedCatalog, MergedHook, MergedMcp, MergedPack, MergedSkill,
    load_catalogs, merge_catalogs, qualified_name,
};
use crate::model::config::{CONFIG_FILENAMES, load_project_config};
use crate::model::expectation::Expectation;
use crate::model::hook_entity::HookType;
use crate::model::mcp_entity::McpTransport;
use crate::model::pattern::{PatternEntry, matches_pattern};
use crate::model::requirement::{ITEM_KINDS, ItemKind};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

/// Stands in for a description an item does not declare.
const UNDESCRIBED: &str = "-";

/// An object built in the order its keys are inserted.
#[derive(Default)]
struct Record(JsonObject);

impl Record {
    fn with(mut self, key: &str, value: impl Into<JsonValue>) -> Self {
        self.0.insert(key.to_owned(), value.into());
        self
    }

    fn with_some(self, key: &str, value: Option<impl Into<JsonValue>>) -> Self {
        match value {
            Some(value) => self.with(key, value),
            None => self,
        }
    }

    fn done(self) -> JsonValue {
        JsonValue::Object(self.0)
    }
}

/// A map of strings as a JSON object, in its own order.
fn string_map(map: &indexmap::IndexMap<String, String>) -> JsonValue {
    JsonValue::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), JsonValue::String(value.clone())))
            .collect(),
    )
}

/// A `requires` list as a record, with namespace and pattern kept apart as the document writes
/// them.
///
/// No `catalog` key: a catalog's own entry carries no qualifier, since it resolves within the
/// catalog the record is already keyed by.
fn requires_json(requires: &[PatternEntry]) -> JsonValue {
    JsonValue::Array(
        requires
            .iter()
            .map(|entry| {
                Record::default()
                    .with("kind", entry.kind.as_str())
                    .with("pattern", entry.pattern.clone())
                    .done()
            })
            .collect(),
    )
}

fn expects_json(expects: &[Expectation]) -> JsonValue {
    JsonValue::Array(
        expects
            .iter()
            .map(|item| {
                Record::default()
                    .with("kind", item.kind.as_str())
                    .with("name", item.name.clone())
                    .done()
            })
            .collect(),
    )
}

/// One pack: what it is for, and what asking for it gets you.
fn pack_json(pack: &MergedPack) -> JsonValue {
    Record::default()
        .with("catalog", pack.catalog.clone())
        .with_some("description", pack.description.clone())
        .with("requires", requires_json(&pack.requires))
        .done()
}

fn transport_json(transport: &McpTransport) -> JsonValue {
    match transport {
        McpTransport::Stdio(stdio) => Record::default()
            .with("args", stdio.args.clone())
            .with("command", stdio.command.clone())
            .with("env", string_map(&stdio.env))
            .with("kind", transport.kind().as_str())
            .done(),
        McpTransport::Http(http) => Record::default()
            .with_some("bearer_token_env_var", http.bearer_token_env_var.clone())
            .with("headers", string_map(&http.headers))
            .with("kind", transport.kind().as_str())
            .with("url", http.url.clone())
            .done(),
    }
}

fn skill_json(skill: &MergedSkill) -> JsonValue {
    Record::default()
        .with("catalog", skill.catalog.clone())
        .with_some("description", skill.description.clone())
        .with("expects", expects_json(&skill.expects))
        .with("path", skill.path.clone())
        .with("requires", requires_json(&skill.requires))
        .done()
}

fn mcp_json(mcp: &MergedMcp) -> JsonValue {
    Record::default()
        .with("catalog", mcp.catalog.clone())
        .with("expects", expects_json(&mcp.expects))
        .with("transport", transport_json(&mcp.transport))
        .done()
}

/// One hook, including the catalog that provided it, which the document itself doesn't say.
fn hook_json(hook: &MergedHook) -> JsonValue {
    Record::default()
        .with("catalog", hook.catalog.clone())
        .with("command", hook.command.clone())
        .with_some("description", hook.description.clone())
        .with("event", hook.event.as_str())
        .with("expects", expects_json(&hook.expects))
        .with_some("matcher", hook.matcher.clone())
        .with("path", hook.path.clone())
        .with_some("timeout", hook.timeout)
        .with("type", hook.r#type.as_str())
        .done()
}

fn to_json(merged: &MergedCatalog) -> JsonValue {
    Record::default()
        .with("catalogs", merged.catalogs.clone())
        .with(
            "hooks",
            keyed(
                &merged.hooks,
                |hook| qualified_name(&hook.catalog, &hook.name),
                hook_json,
            ),
        )
        .with(
            "mcps",
            keyed(
                &merged.mcps,
                |mcp| qualified_name(&mcp.catalog, &mcp.name),
                mcp_json,
            ),
        )
        .with(
            "packs",
            keyed(
                &merged.packs,
                |pack| qualified_name(&pack.catalog, &pack.name),
                pack_json,
            ),
        )
        .with(
            "skills",
            keyed(
                &merged.skills,
                |skill| qualified_name(&skill.catalog, &skill.name),
                skill_json,
            ),
        )
        .done()
}

fn transport_summary(transport: &McpTransport) -> String {
    match transport {
        McpTransport::Stdio(stdio) => {
            let mut words = vec![stdio.command.clone()];

            words.extend(stdio.args.iter().cloned());
            format!("stdio: {}", words.join(" "))
        }
        McpTransport::Http(http) => format!("http: {}", http.url),
    }
}

/// What the hook runs. Marks a shipped script explicitly, since the name alone doesn't say.
fn command_summary(hook: &MergedHook) -> String {
    if hook.r#type == HookType::Script {
        format!("{} (shipped)", hook.command)
    } else {
        hook.command.clone()
    }
}

/// The three filters, already checked against the project, in the form the walk below wants them.
///
/// A filter absent from the command line is an empty set rather than a set holding everything;
/// every test is `set.is_empty() || set.contains(x)`. Expanding an absent `--capability` into all
/// four kinds would make "asked for nothing" and "asked for all four" indistinguishable.
struct SearchFilter {
    /// The glob every result's name must match. Always present: the pattern is a required
    /// argument.
    pattern: String,
    /// Which namespaces to search. Empty means every one of them.
    capabilities: IndexSet<ItemKind>,
    /// Which catalogs to search. Empty means every one of them.
    catalogs: IndexSet<String>,
}

/// The filters as the command line gave them, with every `--catalog` checked against what the
/// project actually lists.
///
/// An unknown catalog alias is exit 2 rather than an empty result. Otherwise `--catalog acme` when
/// the config spells it `acme-core` would produce an empty listing indistinguishable from a catalog
/// with no matching items. The flag that names a thing is checked; the flag that describes a thing
/// (the pattern) is not, since a pattern matching nothing is a legitimate answer.
///
/// # Errors
///
/// Exit 2 when a `--catalog` names no catalog in `ambit.yml`.
fn filter_of(ctx: &CommandContext<'_>, configured: &[String]) -> Result<SearchFilter> {
    let requested = list_of(ctx, "catalog");

    if let Some(unknown) = requested.iter().find(|name| !configured.contains(name)) {
        let file = CONFIG_FILENAMES[0];

        return Err(config_error(
            format!("no catalog named \"{unknown}\" ({file})"),
            if configured.is_empty() {
                vec![
                    format!("{file} lists no catalogs"),
                    "add one under `catalogs:`, or drop --catalog".to_owned(),
                ]
            } else {
                vec![
                    format!("this project lists: {}", configured.join(", ")),
                    "name one of those, or drop --catalog to search every catalog".to_owned(),
                ]
            },
        ));
    }

    // Filtered out of `ITEM_KINDS` rather than built from what was typed, so the set is `ItemKind`
    // without a parse. The CLI already refuses a value that is not one of the four.
    let capabilities = list_of(ctx, "capability");

    Ok(SearchFilter {
        pattern: ctx.args.first().cloned().unwrap_or_default(),
        capabilities: ITEM_KINDS
            .iter()
            .copied()
            .filter(|kind| capabilities.iter().any(|given| given == kind.as_str()))
            .collect(),
        catalogs: requested.iter().cloned().collect(),
    })
}

/// Whether this namespace was asked for. Empty means every one of them; see [`SearchFilter`].
fn wants(filter: &SearchFilter, kind: ItemKind) -> bool {
    filter.capabilities.is_empty() || filter.capabilities.contains(&kind)
}

/// Whether an item survives the pattern and the `--catalog` filter.
///
/// `--capability` is not applied here: it decides whether a whole section is printed, not whether
/// an item survives. Applying it per item would still leave `--capability skill` printing three
/// empty sections it was told not to show.
fn survives(filter: &SearchFilter, name: &str, catalog: &str) -> bool {
    matches_pattern(&filter.pattern, name)
        && (filter.catalogs.is_empty() || filter.catalogs.contains(catalog))
}

/// The items of one namespace that survive, or none if `--capability` excluded the namespace.
fn matching<T: Clone>(
    items: &[T],
    filter: &SearchFilter,
    kind: ItemKind,
    key: impl Fn(&T) -> (&str, &str),
) -> Vec<T> {
    if !wants(filter, kind) {
        return Vec::new();
    }

    items
        .iter()
        .filter(|item| {
            let (name, catalog) = key(item);

            survives(filter, name, catalog)
        })
        .cloned()
        .collect()
}

/// The merged catalog narrowed to what the filters asked for.
///
/// A namespace `--capability` excluded becomes empty here rather than absent, so both output modes
/// take the same shape from the same value: the text form asks [`wants`] which sections to print,
/// and the JSON form always emits all four keys, so a script reading `.skills` never has to check
/// whether the key exists.
fn narrow(merged: &MergedCatalog, filter: &SearchFilter) -> MergedCatalog {
    MergedCatalog {
        catalogs: if filter.catalogs.is_empty() {
            merged.catalogs.clone()
        } else {
            merged
                .catalogs
                .iter()
                .filter(|name| filter.catalogs.contains(name.as_str()))
                .cloned()
                .collect()
        },
        packs: matching(&merged.packs, filter, ItemKind::Pack, |item| {
            (&item.name, &item.catalog)
        }),
        skills: matching(&merged.skills, filter, ItemKind::Skill, |item| {
            (&item.name, &item.catalog)
        }),
        mcps: matching(&merged.mcps, filter, ItemKind::Mcp, |item| {
            (&item.name, &item.catalog)
        }),
        hooks: matching(&merged.hooks, filter, ItemKind::Hook, |item| {
            (&item.name, &item.catalog)
        }),
    }
}

fn to_text(catalogs: &[Catalog], merged: &MergedCatalog, filter: &SearchFilter) -> Vec<String> {
    // The catalogs actually searched, so the header answers "where did I just look", not "what
    // does this project list". The two differ when `--catalog` was given.
    let searched: Vec<&Catalog> = catalogs
        .iter()
        .filter(|catalog| filter.catalogs.is_empty() || filter.catalogs.contains(&catalog.name))
        .collect();
    let mut lines: Vec<String> = if searched.is_empty() {
        vec!["no catalogs configured".to_owned(), String::new()]
    } else {
        searched
            .iter()
            .map(|catalog| format!("{}  {}", catalog.name, catalog.source))
            .chain(std::iter::once(String::new()))
            .collect()
    };

    // Packs first, and carrying their descriptions, since this is the list of things there are
    // names for.
    if wants(filter, ItemKind::Pack) {
        let rows: Vec<Vec<String>> = merged
            .packs
            .iter()
            .map(|pack| {
                vec![
                    pack.name.clone(),
                    pack.catalog.clone(),
                    pack.description
                        .clone()
                        .unwrap_or_else(|| UNDESCRIBED.to_owned()),
                ]
            })
            .collect();

        lines.extend(section("packs", &rows));
    }

    if wants(filter, ItemKind::Skill) {
        let rows: Vec<Vec<String>> = merged
            .skills
            .iter()
            .map(|skill| vec![skill.name.clone(), skill.catalog.clone()])
            .collect();

        lines.extend(section("skills", &rows));
    }

    if wants(filter, ItemKind::Mcp) {
        let rows: Vec<Vec<String>> = merged
            .mcps
            .iter()
            .map(|mcp| {
                vec![
                    mcp.name.clone(),
                    mcp.catalog.clone(),
                    transport_summary(&mcp.transport),
                ]
            })
            .collect();

        lines.extend(section("mcps", &rows));
    }

    if wants(filter, ItemKind::Hook) {
        let rows: Vec<Vec<String>> = merged
            .hooks
            .iter()
            .map(|hook| {
                vec![
                    hook.name.clone(),
                    hook.catalog.clone(),
                    hook.event.as_str().to_owned(),
                    command_summary(hook),
                ]
            })
            .collect();

        lines.extend(section("hooks", &rows));
    }

    lines
}

/// # Errors
///
/// Exit 2 for a missing or malformed config or catalog, or an unknown `--catalog`; exit 4 if a
/// fetch fails.
pub fn search_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let context = source_context_of(ctx);
    let config = load_project_config(&context.project_dir)?;
    let catalogs = load_catalogs(&config, &context, &mut CatalogLoadOptions::default())?;
    let merged = merge_catalogs(&catalogs);
    let filter = filter_of(ctx, &merged.catalogs)?;
    let found = narrow(&merged, &filter);

    if json_requested(ctx) {
        ctx.io.stdout(&stringify_pretty(&to_json(&found)));

        return Ok(ExitCode::Success);
    }

    print_sections(&to_text(&catalogs, &found, &filter), ctx.io);

    Ok(ExitCode::Success)
}
