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

const UNDESCRIBED: &str = "-";

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

fn string_map(map: &indexmap::IndexMap<String, String>) -> JsonValue {
    JsonValue::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), JsonValue::String(value.clone())))
            .collect(),
    )
}

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

fn command_summary(hook: &MergedHook) -> String {
    if hook.r#type == HookType::Script {
        format!("{} (shipped)", hook.command)
    } else {
        hook.command.clone()
    }
}

struct SearchFilter {
    pattern: String,
    capabilities: IndexSet<ItemKind>,
    catalogs: IndexSet<String>,
}

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

fn wants(filter: &SearchFilter, kind: ItemKind) -> bool {
    filter.capabilities.is_empty() || filter.capabilities.contains(&kind)
}

fn survives(filter: &SearchFilter, name: &str, catalog: &str) -> bool {
    matches_pattern(&filter.pattern, name)
        && (filter.catalogs.is_empty() || filter.catalogs.contains(catalog))
}

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
