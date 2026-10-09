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

fn with_reason(mut record: JsonObject, why: Option<String>) -> JsonValue {
    if let Some(why) = why {
        record.insert("reason".to_owned(), JsonValue::String(why));
    }

    JsonValue::Object(record)
}

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

    let mut lines = section("packs", &packs);

    lines.extend(section("skills", &skills));
    lines.extend(section("mcps", &mcps));
    lines.extend(section("hooks", &hooks));
    lines.extend(section("expects", &expects));

    Ok(lines)
}

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
