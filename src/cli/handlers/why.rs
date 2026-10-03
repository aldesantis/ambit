//! `ambit why <kind>:<name>`: explain why one item is in the bundle.
//!
//! Prints the full chain, not just the immediate reason: being told a skill arrived through
//! `required-by:acme-brief` only moves the question up one level. The `requires` entry at the far
//! end is what a reader can actually change.
//!
//! A bundle item is the only valid subject. An `expects` entry is not one: nothing provides an
//! environment variable, so there is no chain to walk. Whether the machine satisfies an
//! expectation is `doctor`'s question.
//!
//! The subject must declare its namespace (`mcp:sentry`); a bare name is refused rather than looked
//! up. This keeps this command and a `requires` entry from disagreeing about what a bare name
//! means: `skill:mcp.sentry` and `mcp:sentry` stay distinct and both askable.
//!
//! A name that resolves to nothing selected is an error, not an empty report, since "not in the
//! bundle" is a resolution answer a script needs to detect. No catalog providing it, versus nothing
//! selecting it, are different problems with different fixes.

use crate::cli::commands::{CommandContext, json_requested, source_context_of};
use crate::cli::output::{print_sections, section};
use crate::errors::{AmbitError, ExitCode, Result, resolution_error};
use crate::model::catalog::{CatalogLoadOptions, MergedCatalog, load_catalogs, merge_catalogs};
use crate::model::config::{ProjectConfig, load_project_config};
use crate::model::pattern::{PatternEntry, REQUIRES_KEY, entry_yaml};
use crate::model::requirement::parse_item_subject;
use crate::resolution::resolve::{
    Bundle, BundleItem, ItemKind, ReasonedItem, SelectionReason, explain_selection, format_reason,
    is_selected, reason_of, resolve_bundle,
};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

/// How an item is named in messages, one arm per namespace so a fifth is a compile error.
fn subject_label(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Pack => "pack",
        ItemKind::Skill => "skill",
        ItemKind::Mcp => "MCP server",
        ItemKind::Hook => "hook",
    }
}

/// The same four with an article, for a sentence that needs one.
fn noun(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Pack => "a pack",
        ItemKind::Skill => "a skill",
        ItemKind::Mcp => "an MCP server",
        ItemKind::Hook => "a hook",
    }
}

/// How an item is named in messages.
fn subject(item: &BundleItem) -> String {
    format!("{} \"{}\"", subject_label(item.kind), item.name)
}

/// The catalog of every merged-catalog entry a candidate names. Several, where more than one
/// catalog provides the name.
///
/// Returns all of them, not the first: no catalog takes precedence, and naming one copy when two
/// catalogs ship it would leave a reader editing the wrong catalog. Ordered as the merged catalog
/// orders them.
fn providers<'m>(merged: &'m MergedCatalog, item: &BundleItem) -> Vec<&'m str> {
    let name = item.name.as_str();

    match item.kind {
        ItemKind::Pack => merged
            .packs
            .iter()
            .filter(|pack| pack.name == name)
            .map(|pack| pack.catalog.as_str())
            .collect(),
        ItemKind::Skill => merged
            .skills
            .iter()
            .filter(|skill| skill.name == name)
            .map(|skill| skill.catalog.as_str())
            .collect(),
        ItemKind::Mcp => merged
            .mcps
            .iter()
            .filter(|mcp| mcp.name == name)
            .map(|mcp| mcp.catalog.as_str())
            .collect(),
        ItemKind::Hook => merged
            .hooks
            .iter()
            .filter(|hook| hook.name == name)
            .map(|hook| hook.catalog.as_str())
            .collect(),
    }
}

/// The `requires` entry that would select an item, written out for the reader to paste.
///
/// By exact name and qualified with the catalog that provides it: that is the one entry guaranteed
/// to select this copy and nothing else. A wildcard could reach other items under the same prefix.
fn selection_entry(item: &BundleItem, catalog: &str) -> String {
    entry_yaml(&PatternEntry {
        kind: item.kind,
        pattern: item.name.clone(),
        catalog: Some(catalog.to_owned()),
    })
}

/// The error for an item one or more catalogs provide but nothing selects.
///
/// Names every catalog it could have come from, so a reader knows the config is otherwise fine,
/// and ends on an entry they can paste.
///
/// The advice is one entry, on the first providing catalog in merged order. There's only one route
/// into a bundle: a pack, a hook, and a server are addressable exactly as a skill is.
///
/// The entry names the item directly. That's always a valid answer, though not always the one the
/// catalog intends (an item a pack already covers is more naturally taken by requiring the pack).
/// Finding that pack would mean searching every pack's `requires` for a match, so the direct entry
/// is offered instead; `ambit search --capability pack "*"` is where to look for packs.
fn not_selected(item: &BundleItem, catalogs: &[&str], config: &ProjectConfig) -> AmbitError {
    let names = catalogs
        .iter()
        .map(|catalog| format!("\"{catalog}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let provided = if catalogs.len() == 1 {
        format!("catalog {names} provides it")
    } else {
        format!("catalogs {names} provide it")
    };

    resolution_error(
        format!("{} is not in the bundle", subject(item)),
        [
            format!(
                "{provided}, but no `{REQUIRES_KEY}` entry in {} selects it",
                config.origin.file
            ),
            format!("select it with `{}`", selection_entry(item, catalogs[0])),
        ],
    )
}

/// The error for a reference nothing provides at all. Names the specific namespace asked for.
fn unknown_name(item: &BundleItem, config: &ProjectConfig) -> AmbitError {
    resolution_error(
        format!("unknown {}", subject(item)),
        [
            format!(
                "nothing configured in {} provides {} by that name",
                config.origin.file,
                noun(item.kind)
            ),
            // Wrapped in wildcards since the likely cause is a name remembered slightly wrong.
            format!(
                "run `ambit search --capability {} \"*{}*\"` to see what is available",
                item.kind, item.name
            ),
        ],
    )
}

/// The bundle item the subject names.
///
/// Taken at its word and never looked up: `mcp:sentry` is the server whether or not a skill of
/// that name exists, so naming the wrong namespace is a miss, not a search that wanders into the
/// right one.
///
/// # Errors
///
/// Exit 2 for a subject naming no namespace; exit 3 for one nothing provides, or one nothing
/// selects.
fn locate(
    name: &str,
    bundle: &Bundle,
    merged: &MergedCatalog,
    config: &ProjectConfig,
) -> Result<BundleItem> {
    let item = parse_item_subject(name, &format!("`why {name}` does not say what to explain"))?;

    if is_selected(bundle, &item) {
        return Ok(item);
    }

    let catalogs = providers(merged, &item);

    if catalogs.is_empty() {
        return Err(unknown_name(&item, config));
    }

    Err(not_selected(&item, &catalogs, config))
}

fn link_json(link: &ReasonedItem) -> JsonValue {
    let mut record = JsonObject::new();

    record.insert("kind".to_owned(), link.kind.as_str().into());
    record.insert("name".to_owned(), link.name.clone().into());
    record.insert("reason".to_owned(), format_reason(&link.reason).into());

    JsonValue::Object(record)
}

fn to_json(item: &BundleItem, chain: &[ReasonedItem], reason: &SelectionReason) -> JsonValue {
    let mut record = JsonObject::new();

    record.insert(
        "chain".to_owned(),
        JsonValue::Array(chain.iter().map(link_json).collect()),
    );
    record.insert("kind".to_owned(), item.kind.as_str().into());
    record.insert("name".to_owned(), item.name.clone().into());
    record.insert("reason".to_owned(), format_reason(reason).into());

    JsonValue::Object(record)
}

fn to_text(item: &BundleItem, chain: &[ReasonedItem]) -> Vec<String> {
    let rows: Vec<Vec<String>> = chain
        .iter()
        .map(|link| {
            vec![
                link.name.clone(),
                link.kind.as_str().to_owned(),
                format_reason(&link.reason),
            ]
        })
        .collect();
    let mut lines = vec![format!("{} {}", item.kind, item.name), String::new()];

    lines.extend(section("chain", &rows));
    lines
}

/// # Errors
///
/// Exit 2 for a missing or malformed config or catalog, or a subject naming no namespace; exit 3
/// for a resolution error or a subject that is not in the bundle; exit 4 if a fetch fails.
pub fn why_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let Some(name) = ctx.args.first().cloned() else {
        // The parser enforces the argument, so this is unreachable rather than a user-facing path.
        return Err(AmbitError::new(
            ExitCode::Internal,
            "`ambit why` was given no name",
            [
                "the command takes the name of a pack, a skill, an MCP server, or a hook",
                "run `ambit why <kind>:<name>`",
            ],
        ));
    };

    let context = source_context_of(ctx);
    let config = load_project_config(&context.project_dir)?;
    let merged = merge_catalogs(&load_catalogs(
        &config,
        &context,
        &mut CatalogLoadOptions::default(),
    )?);
    let bundle = resolve_bundle(&config, &merged)?;

    let item = locate(&name, &bundle, &merged, &config)?;
    let chain = explain_selection(&bundle, &item)?;

    if json_requested(ctx) {
        let reason = reason_of(&bundle, &item)?;

        ctx.io
            .stdout(&stringify_pretty(&to_json(&item, &chain, reason)));

        return Ok(ExitCode::Success);
    }

    print_sections(&to_text(&item, &chain), ctx.io);

    Ok(ExitCode::Success)
}
