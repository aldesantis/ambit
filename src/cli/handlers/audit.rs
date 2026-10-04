//! `ambit audit`: hidden text and risky command lines in every catalog the project lists.
//!
//! One section per kind that has findings, then a summary line. A failure is marked `!` and a
//! warning `~`, so the line that decided the exit code stands out without a second list. A clean
//! audit prints the summary alone, saying how much it read.

use indexmap::{IndexMap, IndexSet};

use crate::cli::commands::{CommandContext, json_requested, source_context_of};
use crate::cli::output::{print_sections, section};
use crate::errors::{ExitCode, Result};
use crate::model::requirement::{CATALOG_SEPARATOR, ITEM_KINDS, ItemKind};
use crate::project::audit::{AuditFinding, AuditReport, AuditSeverity, audit_project, count};
use crate::util::json::{JsonObject, JsonValue, stringify_pretty};

fn finding_json(finding: &AuditFinding) -> JsonValue {
    let mut record = JsonObject::new();

    record.insert("catalog".to_owned(), finding.catalog.clone().into());
    record.insert("check".to_owned(), finding.check.as_str().into());
    record.insert(
        "file".to_owned(),
        finding
            .file
            .clone()
            .map_or(JsonValue::Null, JsonValue::String),
    );
    record.insert("kind".to_owned(), finding.kind.as_str().into());
    record.insert(
        "line".to_owned(),
        finding.line.map_or(JsonValue::Null, JsonValue::from),
    );
    record.insert("message".to_owned(), finding.message.clone().into());
    record.insert("name".to_owned(), finding.name.clone().into());
    record.insert("severity".to_owned(), finding.severity.as_str().into());

    JsonValue::Object(record)
}

fn to_json(report: &AuditReport) -> JsonValue {
    let mut record = JsonObject::new();

    record.insert(
        "findings".to_owned(),
        JsonValue::Array(report.findings.iter().map(finding_json).collect()),
    );
    record.insert("items".to_owned(), report.items.into());
    record.insert("passed".to_owned(), report.passed().into());

    JsonValue::Object(record)
}

/// The section a kind's findings are listed under.
fn section_title(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Pack => "packs",
        ItemKind::Skill => "skills",
        ItemKind::Mcp => "mcps",
        ItemKind::Hook => "hooks",
    }
}

fn to_text(report: &AuditReport) -> Vec<String> {
    if report.findings.is_empty() {
        return vec![
            format!(
                "audit checked {} and found no issues",
                count(report.items, "item", "items")
            ),
            String::new(),
        ];
    }

    // A name two catalogs both ship is qualified with its catalog, so the row says which copy.
    let mut catalogs_of: IndexMap<(ItemKind, &str), IndexSet<&str>> = IndexMap::new();

    for finding in &report.findings {
        catalogs_of
            .entry((finding.kind, &finding.name))
            .or_default()
            .insert(&finding.catalog);
    }

    let mut lines = Vec::new();

    for &kind in ITEM_KINDS {
        let rows: Vec<Vec<String>> = report
            .findings
            .iter()
            .filter(|finding| finding.kind == kind)
            .map(|finding| {
                let mark = match finding.severity {
                    AuditSeverity::Fail => "!",
                    AuditSeverity::Warn => "~",
                };
                let shared = catalogs_of
                    .get(&(finding.kind, finding.name.as_str()))
                    .is_some_and(|catalogs| catalogs.len() > 1);
                let label = if shared {
                    format!("{}{CATALOG_SEPARATOR}{}", finding.catalog, finding.name)
                } else {
                    finding.name.clone()
                };

                vec![mark.to_owned(), label, finding.message.clone()]
            })
            .collect();

        if !rows.is_empty() {
            lines.extend(section(section_title(kind), &rows));
        }
    }

    let items: IndexSet<(ItemKind, &str, &str)> = report
        .findings
        .iter()
        .map(|finding| {
            (
                finding.kind,
                finding.catalog.as_str(),
                finding.name.as_str(),
            )
        })
        .collect();

    lines.push(format!(
        "audit found {} in {}",
        count(report.findings.len(), "issue", "issues"),
        count(items.len(), "item", "items")
    ));
    lines.push(String::new());
    lines
}

/// # Errors
///
/// Whatever [`audit_project`] returns. A finding is never an error: failures exit 6 through the
/// returned code.
pub fn audit_handler(ctx: &mut CommandContext<'_>) -> Result<ExitCode> {
    let report = audit_project(&source_context_of(ctx))?;

    if json_requested(ctx) {
        ctx.io.stdout(&stringify_pretty(&to_json(&report)));
    } else {
        print_sections(&to_text(&report), ctx.io);
    }

    Ok(if report.passed() {
        ExitCode::Success
    } else {
        ExitCode::Doctor
    })
}
