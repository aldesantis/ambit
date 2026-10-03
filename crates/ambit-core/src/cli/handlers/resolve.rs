//! `ambit resolve`: compute the bundle and print it.
//!
//! `--json` is the golden-file surface: no absolute paths, every key emitted in sorted order. The
//! shape mirrors `ambit.lock` minus the parts only a fetched catalog can supply, so the lock is
//! later a serialization of this rather than a second, differently-shaped view.
//!
//! `--explain` adds one column and one key rather than a different report, so a reader comparing
//! the two doesn't need to re-find their bearings. The reason is the short form; `ambit why` prints
//! the whole chain.
//!
//! A bundle holds one item per name; a selection reaching two catalogs' copies of the same name is
//! refused at resolve, not reported here, since both would be installed at one path.

use serde_json::json;

use crate::cli::commands::{CommandContext, json_requested, source_context_of};
use crate::cli::output::{keyed, print_sections, section};
use crate::errors::{ExitCode, Result};
use crate::model::catalog::{CatalogLoadOptions, load_catalogs, merge_catalogs};
use crate::model::config::load_project_config;
use crate::model::expectation::EXPECTATION_KINDS;
use crate::resolution::resolve::{
    Bundle, BundleItem, ItemKind, format_reason, reason_of, resolve_bundle,
};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

/// The reason column and key, present only under `--explain`.
fn reason(bundle: &Bundle, kind: ItemKind, name: &str, explain: bool) -> Result<Option<String>> {
    if !explain {
        return Ok(None);
    }

    let item = BundleItem {
        kind,
        name: name.to_owned(),
    };

    Ok(Some(format_reason(reason_of(bundle, &item)?)))
}

/// A record with `reason` appended when there is one.
fn with_reason(mut record: JsonObject, why: Option<String>) -> JsonValue {
    if let Some(why) = why {
        record.insert("reason".to_owned(), JsonValue::String(why));
    }

    JsonValue::Object(record)
}

/// [`keyed`], for a projection that can fail.
fn try_keyed<T>(
    items: &[T],
    name: impl Fn(&T) -> String,
    value: impl Fn(&T) -> Result<JsonValue>,
) -> Result<JsonObject> {
    let values = items.iter().map(&value).collect::<Result<Vec<_>>>()?;
    let pairs: Vec<(String, JsonValue)> = items.iter().map(name).zip(values).collect();

    Ok(keyed(
        &pairs,
        |(name, _)| name.clone(),
        |(_, value)| value.clone(),
    ))
}

/// An object with the keys in the order given.
fn object<const N: usize>(pairs: [(&str, &str); N]) -> JsonObject {
    pairs
        .into_iter()
        .map(|(key, value)| (key.to_owned(), JsonValue::String(value.to_owned())))
        .collect()
}

fn to_json(bundle: &Bundle, explain: bool) -> Result<JsonValue> {
    let expects: JsonObject = EXPECTATION_KINDS
        .iter()
        .map(|&kind| (kind.as_str().to_owned(), json!(bundle.expects.get(kind))))
        .collect();

    let hooks = try_keyed(
        &bundle.hooks,
        |hook| hook.name.clone(),
        |hook| {
            let why = reason(bundle, ItemKind::Hook, &hook.name, explain)?;

            Ok(with_reason(
                object([("catalog", &hook.catalog), ("event", hook.event.as_str())]),
                why,
            ))
        },
    )?;
    let mcps = try_keyed(
        &bundle.mcps,
        |mcp| mcp.name.clone(),
        |mcp| {
            let why = reason(bundle, ItemKind::Mcp, &mcp.name, explain)?;

            Ok(with_reason(object([("catalog", &mcp.catalog)]), why))
        },
    )?;
    // A pack materializes nothing, so this record carries no path and no bytes: it only says the
    // project asked for it.
    let packs = try_keyed(
        &bundle.packs,
        |pack| pack.name.clone(),
        |pack| {
            let why = reason(bundle, ItemKind::Pack, &pack.name, explain)?;

            Ok(with_reason(object([("catalog", &pack.catalog)]), why))
        },
    )?;
    let skills = try_keyed(
        &bundle.skills,
        |skill| skill.name.clone(),
        |skill| {
            let why = reason(bundle, ItemKind::Skill, &skill.name, explain)?;

            Ok(with_reason(
                object([("catalog", &skill.catalog), ("path", &skill.path)]),
                why,
            ))
        },
    )?;

    let mut record = JsonObject::new();

    record.insert("expects".to_owned(), JsonValue::Object(expects));
    record.insert("hooks".to_owned(), JsonValue::Object(hooks));
    record.insert("mcps".to_owned(), JsonValue::Object(mcps));
    record.insert("packs".to_owned(), JsonValue::Object(packs));
    record.insert("skills".to_owned(), JsonValue::Object(skills));

    Ok(JsonValue::Object(record))
}

/// A row with the reason appended, or the row unchanged when nothing was asked to explain it.
fn row(mut cells: Vec<String>, why: Option<String>) -> Vec<String> {
    cells.extend(why);
    cells
}

fn to_text(bundle: &Bundle, explain: bool) -> Result<Vec<String>> {
    let packs = bundle
        .packs
        .iter()
        .map(|pack| {
            Ok(row(
                vec![pack.name.clone(), pack.catalog.clone()],
                reason(bundle, ItemKind::Pack, &pack.name, explain)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let skills = bundle
        .skills
        .iter()
        .map(|skill| {
            Ok(row(
                vec![skill.name.clone(), skill.catalog.clone()],
                reason(bundle, ItemKind::Skill, &skill.name, explain)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let mcps = bundle
        .mcps
        .iter()
        .map(|mcp| {
            Ok(row(
                vec![mcp.name.clone(), mcp.catalog.clone()],
                reason(bundle, ItemKind::Mcp, &mcp.name, explain)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let hooks = bundle
        .hooks
        .iter()
        .map(|hook| {
            Ok(row(
                vec![
                    hook.name.clone(),
                    hook.catalog.clone(),
                    hook.event.as_str().to_owned(),
                ],
                reason(bundle, ItemKind::Hook, &hook.name, explain)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    // One row per precondition, kind in its own column, so `env` and `bin` entries are
    // distinguishable at a glance.
    let expects: Vec<Vec<String>> = EXPECTATION_KINDS
        .iter()
        .flat_map(|&kind| {
            bundle
                .expects
                .get(kind)
                .iter()
                .map(move |name| vec![kind.as_str().to_owned(), name.clone()])
        })
        .collect();

    // Packs first: they are what a project usually wrote down; the sections below are what they
    // expanded to.
    let mut lines = section("packs", &packs);

    lines.extend(section("skills", &skills));
    lines.extend(section("mcps", &mcps));
    lines.extend(section("hooks", &hooks));
    lines.extend(section("expects", &expects));

    Ok(lines)
}

/// # Errors
///
/// Exit 2 for a missing or malformed config or catalog; exit 3 for a resolution error; exit 4 if a
/// fetch fails.
pub fn resolve_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let explain = ctx.options.flag("explain");

    let context = source_context_of(ctx);
    let config = load_project_config(&context.project_dir)?;
    let merged = merge_catalogs(&load_catalogs(
        &config,
        &context,
        &mut CatalogLoadOptions::default(),
    )?);
    let bundle = resolve_bundle(&config, &merged)?;

    if json_requested(ctx) {
        ctx.io
            .stdout(&stringify_pretty(&to_json(&bundle, explain)?));

        return Ok(ExitCode::Success);
    }

    print_sections(&to_text(&bundle, explain)?, ctx.io);

    Ok(ExitCode::Success)
}
