use serde_json::json;

use crate::cli::commands::{CommandContext, json_requested, source_context_of};
use crate::cli::output::print_sections;
use crate::errors::{ExitCode, Result};
use crate::resolution::validate::{
    ValidationProblem, ValidationReport, is_valid, validate_project,
};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

fn problem_json(problem: &ValidationProblem) -> JsonValue {
    let mut record = JsonObject::new();

    record.insert("detail".to_owned(), json!(problem.detail));
    record.insert("kind".to_owned(), problem.kind.as_str().into());
    record.insert("message".to_owned(), problem.message.clone().into());

    JsonValue::Object(record)
}

fn to_json(report: &ValidationReport) -> JsonValue {
    let mut checked = JsonObject::new();

    checked.insert("hooks".to_owned(), report.checked.hooks.into());
    checked.insert("mcps".to_owned(), report.checked.mcps.into());
    checked.insert("packs".to_owned(), report.checked.packs.into());
    checked.insert("skills".to_owned(), report.checked.skills.into());

    let mut record = JsonObject::new();

    record.insert("checked".to_owned(), JsonValue::Object(checked));
    record.insert(
        "problems".to_owned(),
        JsonValue::Array(report.problems.iter().map(problem_json).collect()),
    );
    record.insert("valid".to_owned(), is_valid(report).into());

    JsonValue::Object(record)
}

fn count(total: usize, noun: &str) -> String {
    format!("{total} {noun}{}", if total == 1 { "" } else { "s" })
}

fn problem_lines(problems: &[ValidationProblem]) -> Vec<String> {
    let mut lines = vec![format!("problems ({})", problems.len())];

    if problems.is_empty() {
        lines.push("  (none)".to_owned());
    }

    for problem in problems {
        lines.push(format!("  {}", problem.message));
        lines.extend(problem.detail.iter().map(|line| format!("      {line}")));
    }

    lines.push(String::new());
    lines
}

fn to_text(report: &ValidationReport) -> Vec<String> {
    let checked = report.checked;
    let mut lines = vec![
        format!(
            "checked {}, {}, {}, {}",
            count(checked.packs, "pack"),
            count(checked.skills, "skill"),
            count(checked.mcps, "mcp"),
            count(checked.hooks, "hook")
        ),
        String::new(),
    ];

    lines.extend(problem_lines(&report.problems));
    lines
}

pub fn validate_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let found = validate_project(&source_context_of(ctx))?;

    if json_requested(ctx) {
        ctx.io.stdout(&stringify_pretty(&to_json(&found)));
    } else {
        print_sections(&to_text(&found), ctx.io);
    }

    Ok(if is_valid(&found) {
        ExitCode::Success
    } else {
        ExitCode::Resolution
    })
}
