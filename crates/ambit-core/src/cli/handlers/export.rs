//! `ambit export`: selected packs as Claude plugins.

use crate::cli::commands::{CommandContext, dry_run_requested, json_requested, source_context_of};
use crate::errors::{ExitCode, Result, config_error};
use crate::export::{ExportOptions, ExportResult, export_plugins};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

fn to_json(result: &ExportResult) -> JsonValue {
    let plugins = result
        .plugins
        .iter()
        .map(|plugin| {
            let mut record = JsonObject::new();

            record.insert("name".to_owned(), plugin.name.clone().into());
            record.insert("directory".to_owned(), plugin.directory.clone().into());
            record.insert("files".to_owned(), plugin.files.into());
            JsonValue::Object(record)
        })
        .collect();
    let mut record = JsonObject::new();

    record.insert("output".to_owned(), result.output.clone().into());
    record.insert("plugins".to_owned(), JsonValue::Array(plugins));
    JsonValue::Object(record)
}

/// # Errors
///
/// Whatever the command's own operation returns, already in the standard message shape.
pub fn export_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let (Some("claude-plugin"), Some(output)) =
        (ctx.options.value("format"), ctx.options.value("output"))
    else {
        return Err(config_error(
            "export requires --format claude-plugin and --output <dir>",
            ["run `ambit export --format claude-plugin --output dist/plugins`"],
        ));
    };

    let check = ctx.options.flag("check");
    let dry_run = dry_run_requested(ctx);
    let result = export_plugins(
        &source_context_of(ctx),
        &ExportOptions {
            output: output.to_owned(),
            dry_run,
            link: ctx.options.flag("link"),
            force: ctx.options.flag("force"),
            check,
        },
    )?;

    if json_requested(ctx) {
        ctx.io.stdout(&stringify_pretty(&to_json(&result)));
    } else {
        let verb = if check {
            "Verified"
        } else if dry_run {
            "Would export"
        } else {
            "Exported"
        };

        ctx.io.stdout(&format!(
            "{verb} {} Claude plugins to {}",
            result.plugins.len(),
            result.output
        ));

        for plugin in &result.plugins {
            ctx.io.stdout(&format!(
                "  {}/ ({}, {} files)",
                plugin.directory, plugin.name, plugin.files
            ));
        }
    }

    Ok(ExitCode::Success)
}
