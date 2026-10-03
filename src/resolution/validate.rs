//! Full-catalog validation for `ambit validate`, which is what CI runs.
//!
//! Every item in the merged catalog is checked, whether anything selects it or not, including
//! every catalog's own copy of a name. `resolve` and `install` validate only the selected closure,
//! so a broken skill nobody selects can otherwise sit undetected until the first profile that
//! reaches it fails; this module closes that gap.
//!
//! Problems are collected, not returned on the first: the command prints the whole list and exits 3
//! once. Messages reuse the same builders resolution raises (`unmatched_entry_error` and
//! `cycle_error`), so a problem reads the same whether listed here or raised there.
//!
//! A catalog that fails to parse still exits 2 immediately. The one exception is a skill whose
//! `name` disagrees with its path, which is collected instead (see
//! [`CatalogParseOptions`](crate::model::catalog::CatalogParseOptions)).

use crate::errors::Result;
use crate::model::catalog::MergedCatalog;
use crate::model::config::ProjectConfig;
use crate::model::sources::SourceContext;
use crate::util::string_enum;

string_enum! {
    /// What kind of problem a report entry is, so `--json` can be filtered without parsing prose.
    ///
    /// `UnmatchedPattern` covers any `requires` entry that resolves to nothing: a project's entry
    /// against its configured catalogs, or a skill's own entry against the catalog that ships it.
    /// `UnselectedCatalog` is the one finding whose subject is the config alone.
    pub enum ValidationProblemKind {
        Cycle => "cycle",
        NameMismatch => "name-mismatch",
        UnmatchedPattern => "unmatched-pattern",
        UnselectedCatalog => "unselected-catalog",
    }
}

/// Every validation problem kind, in declaration order.
pub const VALIDATION_PROBLEM_KINDS: &[ValidationProblemKind] = ValidationProblemKind::ALL;

/// One problem, in the shape required of an error, since that is what it would have been.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationProblem {
    pub kind: ValidationProblemKind,
    /// The summary: the offending identifier, and the file it is written in.
    pub message: String,
    /// The remaining lines, ending in one concrete next step.
    pub detail: Vec<String>,
}

/// What the run covered, so a clean report says what it checked rather than saying nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValidationCounts {
    pub hooks: usize,
    pub mcps: usize,
    pub packs: usize,
    pub skills: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidationReport {
    pub checked: ValidationCounts,
    /// Every problem found, in a fixed order: the catalog's own integrity first (name mismatches,
    /// its own `requires` entries, cycles), then how the project uses it (unmatched entries, then
    /// catalogs it never reaches into). Within each check, findings are in name order, or in
    /// document order where the subject is the project's own list.
    pub problems: Vec<ValidationProblem>,
}

/// Whether the catalog is valid: no problems at all, of any kind.
pub fn is_valid(report: &ValidationReport) -> bool {
    report.problems.is_empty()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidateOptions {
    /// The project's config. Every report is about a project, because a catalog repo is one too.
    pub config: ProjectConfig,
    /// Problems collected while parsing, listed ahead of the rest.
    pub parsed: Vec<ValidationProblem>,
    /// The names of the catalogs the project is: the ones whose root resolved to the project
    /// directory itself, which is what `source: path:.` means. Empty means the project publishes
    /// nothing.
    pub own: Vec<String>,
}

/// Validates a merged catalog against the config that assembled it. Pure: it reads only the parsed
/// catalog and the parsed config.
pub fn validate_catalog(merged: &MergedCatalog, options: &ValidateOptions) -> ValidationReport {
    let _ = (merged, options);
    todo!("port resolution/validate.ts:validateCatalog")
}

/// Validates everything a project configures: every catalog it lists, its own items among them,
/// and its own `requires` entries. The only entry point.
///
/// # Errors
///
/// Exit 2 for a missing or malformed config, an unresolvable source, or a catalog that does not
/// parse; exit 4 if a fetch fails.
pub fn validate_project(context: &SourceContext) -> Result<ValidationReport> {
    let _ = context;
    todo!("port resolution/validate.ts:validateProject")
}
