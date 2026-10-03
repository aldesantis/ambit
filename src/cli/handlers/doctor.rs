//! `ambit doctor`: env vars, drift, ownership.
//!
//! Three sections, answering two questions. The `checks` table says what was examined and how each
//! one came out, so a healthy project says so explicitly rather than printing an empty list.
//! Failures and warnings are separated because the split is the exit code: everything under
//! `failures` is why this run returned 6, everything under `warnings` is why it did not.
//!
//! Findings print the way `validate` prints a problem: summary indented like a row, detail lines
//! indented under it as an error's own `format()` does.

use serde_json::json;

use crate::cli::commands::{CommandContext, json_requested, offline_requested, project_dir_of};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result};
use crate::project::doctor::{
    DoctorFinding, DoctorOptions, DoctorReport, diagnose_project, doctor_failures, doctor_warnings,
    is_healthy,
};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

fn finding_json(finding: &DoctorFinding) -> JsonValue {
    let mut record = JsonObject::new();

    record.insert("check".to_owned(), finding.check.as_str().into());
    record.insert("detail".to_owned(), json!(finding.detail));
    record.insert("message".to_owned(), finding.message.clone().into());
    record.insert("severity".to_owned(), finding.severity.as_str().into());

    JsonValue::Object(record)
}

fn to_json(report: &DoctorReport) -> JsonValue {
    let checks = report
        .checks
        .iter()
        .map(|result| {
            let mut record = JsonObject::new();

            record.insert("check".to_owned(), result.check.as_str().into());
            record.insert("status".to_owned(), result.status.as_str().into());

            JsonValue::Object(record)
        })
        .collect();

    let mut record = JsonObject::new();

    record.insert("checks".to_owned(), JsonValue::Array(checks));
    record.insert(
        "findings".to_owned(),
        JsonValue::Array(report.findings.iter().map(finding_json).collect()),
    );
    record.insert("healthy".to_owned(), is_healthy(report).into());

    JsonValue::Object(record)
}

/// Column padding is deliberately absent from the body: these are sentences, not a table.
fn finding_lines(title: &str, findings: &[DoctorFinding]) -> Vec<String> {
    let mut lines = vec![format!("{title} ({})", findings.len())];

    if findings.is_empty() {
        lines.push("  (none)".to_owned());
    }

    for finding in findings {
        lines.push(format!("  {}", finding.message));
        lines.extend(finding.detail.iter().map(|line| format!("      {line}")));
    }

    lines.push(String::new());
    lines
}

fn to_text(report: &DoctorReport) -> Vec<String> {
    let rows: Vec<Vec<String>> = report
        .checks
        .iter()
        .map(|result| {
            vec![
                result.check.as_str().to_owned(),
                result.status.as_str().to_owned(),
            ]
        })
        .collect();
    let mut lines = section("checks", &rows);

    lines.extend(finding_lines("failures", &doctor_failures(report)));
    lines.extend(finding_lines("warnings", &doctor_warnings(report)));
    lines
}

/// # Errors
///
/// Whatever [`diagnose_project`] returns. A finding is never an error: failures exit 6 through
/// the returned code.
pub fn doctor_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let report = diagnose_project(
        &project_dir_of(ctx),
        ctx.env,
        DoctorOptions {
            offline: offline_requested(ctx),
        },
    )?;

    if json_requested(ctx) {
        ctx.io.stdout(&stringify_pretty(&to_json(&report)));
    } else {
        print_sections(&to_text(&report), ctx.io);
    }

    Ok(if is_healthy(&report) {
        ExitCode::Success
    } else {
        ExitCode::Doctor
    })
}
