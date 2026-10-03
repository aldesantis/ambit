//! `ambit validate`, for CI: one report over one subject.
//!
//! Validates everything a project configures: every catalog it lists, and its own `requires`
//! entries. This also covers a catalog repo, since a catalog repo lists itself as `source: path:.`,
//! so its `packs/`, `skills/`, `mcps/` and `hooks/` arrive as an ordinary catalog and get checked
//! the same way.
//!
//! The report is printed and the exit code carries the verdict, like `status --check`: a catalog
//! with problems is a finding, not a failure of ambit's, and a CI job needs both the list to fix
//! and the code to fail on.
//!
//! A clean run still prints what it checked, since "no problems found" alone would be
//! indistinguishable from a run that found nothing to look at (e.g. a catalog whose skills all
//! failed to be discovered).

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

/// The problems block: each summary indented like a section row, each detail line indented under
/// it the way an error's own `format()` does. Column padding is deliberately absent: these are
/// sentences, not a table.
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

/// `ambit validate`: everything the project configures, the project's own catalog included.
///
/// # Errors
///
/// Exit 2 for a missing or malformed config, an unresolvable source, or a catalog that does not
/// parse; exit 4 if a fetch fails. Problems are a report, not an error: they exit 3 through the
/// returned code.
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
