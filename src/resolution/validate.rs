//! Full-catalog validation for `ambit validate`, which is what CI runs. Covers a project and a
//! catalog repo with one report shape: a catalog repo scaffolded by `ambit init` lists itself via a
//! `catalogs:` entry naming `path:.`, whose `skills/`, `mcps/` and `hooks/` are read as the `local`
//! catalog.
//!
//! Every item in the merged catalog is checked, whether anything selects it or not, including
//! every catalog's own copy of a name (two copies of one name are two documents that can each be
//! broken independently). `resolve` and `install` validate only the selected closure, so a broken
//! skill nobody selects can otherwise sit undetected for weeks until the first profile that reaches
//! it fails; this module closes that gap. Everything is checked by name, so a misspelled grouping
//! cannot silently become a new one.
//!
//! Problems are collected, not returned on the first: the command prints the whole list and exits 3
//! once, instead of stopping at the first offender. Messages reuse the same builders resolution
//! raises (`unmatched_entry_error` and `cycle_error`), so a problem reads the same whether listed
//! here or raised there.
//!
//! A catalog that fails to parse still exits 2 immediately. The one exception is a skill whose
//! `name` disagrees with its path, which is collected instead, because the path already answers
//! what the skill is called (see
//! [`CatalogParseOptions`](crate::model::catalog::CatalogParseOptions)).

use std::collections::HashSet;

use indexmap::IndexMap;

use crate::errors::{AmbitError, Result, at, resolution_error};
use crate::model::catalog::{
    CatalogLoadOptions, MergedCatalog, load_catalogs, merge_catalogs, qualified_name,
};
use crate::model::config::{ProjectConfig, load_project_config};
use crate::model::pattern::{PatternEntry, REQUIRES_KEY};
use crate::model::requirement::CATALOG_SEPARATOR;
use crate::model::sources::SourceContext;
use crate::resolution::resolve::{
    BundleItem, Requirer, cycle_error, entry_catalog, entry_position, matches_anything,
    matches_own_catalog, required_entries, required_items, requirer_position, requirers_of,
    unmatched_entry_error,
};
use crate::util::cmp::js_cmp;
use crate::util::path::normalize;
use crate::util::string_enum;

string_enum! {
    /// What kind of problem a report entry is, so `--json` can be filtered without parsing prose.
    ///
    /// `UnmatchedPattern` covers any `requires` entry that resolves to nothing: a project's entry
    /// against its configured catalogs, or a skill's own entry against the catalog that ships it.
    /// An exact name is a pattern with no wildcard, so a misspelled name, a stale glob, and a
    /// dangling requirement are all this one kind.
    ///
    /// `UnselectedCatalog` is the one finding whose subject is the config alone rather than a
    /// document in a catalog; see `unselected_catalog_problems`.
    ///
    /// A name two catalogs provide is not itself a problem here. It is a problem only when a
    /// project selects both copies, which is resolution's judgement to make (see
    /// [`assert_no_collisions`](crate::resolution::resolve::assert_no_collisions)).
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
    /// document order where the subject is the project's own list, so the report is a function of
    /// the inputs, not of read order.
    pub problems: Vec<ValidationProblem>,
}

/// Whether the catalog is valid: no problems at all, of any kind.
pub fn is_valid(report: &ValidationReport) -> bool {
    report.problems.is_empty()
}

/// Wraps what would have been returned as an error as what is listed instead.
fn problem(kind: ValidationProblemKind, error: AmbitError) -> ValidationProblem {
    ValidationProblem {
        kind,
        message: error.message,
        detail: error.detail,
    }
}

/// The refusal a catalog's `requires` entry earns when it selects nothing, built exactly as
/// resolution builds it.
///
/// The catalog the entry resolves in is always the requirer's own: a catalog author cannot write a
/// consumer's alias, so a bare pattern inside a catalog means this catalog.
fn unmatched_requirement(
    requirer: &Requirer,
    entry: &PatternEntry,
    catalogs: &[String],
) -> ValidationProblem {
    problem(
        ValidationProblemKind::UnmatchedPattern,
        unmatched_entry_error(
            entry,
            &requirer.catalog,
            &requirer_position(requirer),
            catalogs,
        ),
    )
}

/// Every `requires` entry inside a catalog that selects nothing, across the whole catalog, not only
/// the closure a project's own entries reach.
///
/// A requirement is a pattern, resolved within the catalog that ships the requiring skill, so an
/// entry satisfied by a sibling catalog's copy is not satisfied. Two catalogs shipping copies of
/// one broken skill therefore yield one finding each, since each is a separate document to fix.
fn requirement_problems(merged: &MergedCatalog) -> Vec<ValidationProblem> {
    let mut problems = Vec::new();

    for requirer in requirers_of(merged) {
        for entry in required_entries(&requirer) {
            if matches_own_catalog(&entry, &requirer, merged) {
                continue;
            }

            problems.push(unmatched_requirement(&requirer, &entry, &merged.catalogs));
        }
    }

    problems
}

/// Node identity: namespace, catalog and name. A pack and a skill may share a name, so the
/// namespace is part of it; a loop through one is not a loop through the other.
fn node_key(kind: &str, catalog: &str, name: &str) -> String {
    format!("{kind}:{}", qualified_name(catalog, name))
}

fn requirer_key(requirer: &Requirer) -> String {
    node_key(requirer.kind.as_str(), &requirer.catalog, &requirer.name)
}

/// The cycle hunt's walk state; see [`cycle_problems`].
struct CycleHunt<'m> {
    merged: &'m MergedCatalog,
    problems: Vec<ValidationProblem>,
    reported: HashSet<String>,
    walked: Vec<Requirer>,
    closed: HashSet<String>,
    by_key: IndexMap<String, Requirer>,
}

impl CycleHunt<'_> {
    fn record(&mut self, cycle: &[Requirer], requirer: &Requirer, entry: &PatternEntry) {
        // The path closes on the node it opened with, so the loop's members are all but the last.
        let members: Vec<String> = cycle[..cycle.len() - 1].iter().map(requirer_key).collect();
        let start = members
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| js_cmp(a, b))
            .map_or(0, |(index, _)| index);
        // U+0000 as separator, written as an escape: a literal NUL byte would make grep and similar
        // tools treat this file as binary.
        let rotated = members[start..]
            .iter()
            .chain(&members[..start])
            .cloned()
            .collect::<Vec<_>>()
            .join("\u{0000}");

        if !self.reported.insert(rotated) {
            return;
        }

        // `<kind>:<name>` in the printed path, full addresses in the key: a path is read against
        // the `requires` lists an author wrote, which name siblings and never qualify them.
        let path: Vec<BundleItem> = cycle
            .iter()
            .map(|seen| BundleItem {
                kind: seen.kind.item_kind(),
                name: seen.name.clone(),
            })
            .collect();

        self.problems.push(problem(
            ValidationProblemKind::Cycle,
            cycle_error(&path, requirer, entry),
        ));
    }

    fn follow(&mut self, requirer: &Requirer) {
        if self.closed.contains(&requirer_key(requirer)) {
            return;
        }

        self.walked.push(requirer.clone());

        for entry in required_entries(requirer) {
            // Packs and skills only: only those two can require anything, so only their edges can
            // close a loop. An entry that selects nothing at all is `requirement_problems`'
            // finding; here it is simply an edge that goes nowhere.
            let selected = required_items(&entry, requirer, self.merged);
            let next: Vec<Requirer> = selected
                .packs
                .iter()
                .map(|pack| node_key("pack", &pack.catalog, &pack.name))
                .chain(
                    selected
                        .skills
                        .iter()
                        .map(|skill| node_key("skill", &skill.catalog, &skill.name)),
                )
                .filter_map(|key| self.by_key.get(&key).cloned())
                .collect();

            for child in next {
                // Checked here rather than on entry to `follow`: the cycle error names the entry
                // that closed the loop, and this is the only place that knows which one that is.
                let child_key = requirer_key(&child);
                let opened = self
                    .walked
                    .iter()
                    .position(|seen| requirer_key(seen) == child_key);

                if let Some(opened) = opened {
                    let mut cycle = self.walked[opened..].to_vec();

                    cycle.push(child);
                    self.record(&cycle, requirer, &entry);
                    continue;
                }

                self.follow(&child);
            }
        }

        self.walked.pop();
        self.closed.insert(requirer_key(requirer));
    }
}

/// Every `requires` cycle, walked from every pack and every skill rather than from the ones a
/// project selects.
///
/// One cycle is reported per back edge the walk meets: two independent cycles are both reported,
/// but two sharing a back edge collapse into the one closed first (still enough to name an edge to
/// remove).
///
/// The walk visits requirers in a fixed order (packs first, then skills, each by name) and never
/// re-follows one it closed, so which member a reported path opens on is deterministic. The
/// canonical-rotation guard keeps a loop reported once regardless of traversal order.
///
/// An edge goes wherever the entry that wrote it selects, within the requirer's own catalog and
/// nowhere else, through [`required_items`] (the same function the closure walks with).
/// Bookkeeping uses full addresses because two catalogs' copies of one name are two nodes, so a
/// loop in each is two separate problems.
fn cycle_problems(merged: &MergedCatalog) -> Vec<ValidationProblem> {
    let mut hunt = CycleHunt {
        merged,
        problems: Vec::new(),
        reported: HashSet::new(),
        walked: Vec::new(),
        closed: HashSet::new(),
        by_key: requirers_of(merged)
            .into_iter()
            .map(|requirer| (requirer_key(&requirer), requirer))
            .collect(),
    };
    let roots: Vec<Requirer> = hunt.by_key.values().cloned().collect();

    for requirer in &roots {
        hunt.follow(requirer);
    }

    hunt.problems
}

/// What a project's own config contributes: every `requires` entry no item satisfies.
///
/// This is already an exit-3 resolution error, which stops at the first offender; listing them all
/// means a config holding four mistyped entries costs one `validate` run instead of four `resolve`
/// runs.
///
/// Listed in the order the entries were written, so the report reads down the file the reader has
/// open. (Resolution itself sorts instead, because it reports only one of them and the choice must
/// not fall to incidental ordering.)
fn config_problems(config: &ProjectConfig, merged: &MergedCatalog) -> Vec<ValidationProblem> {
    config
        .requires
        .iter()
        .filter(|entry| !matches_anything(entry, merged))
        .map(|entry| {
            problem(
                ValidationProblemKind::UnmatchedPattern,
                unmatched_entry_error(
                    entry,
                    entry_catalog(entry),
                    &entry_position(config, entry),
                    &merged.catalogs,
                ),
            )
        })
        .collect()
}

/// How many items one catalog contributed to the merged view, across all four namespaces.
fn item_count(merged: &MergedCatalog, catalog: &str) -> usize {
    merged
        .packs
        .iter()
        .filter(|item| item.catalog == catalog)
        .count()
        + merged
            .skills
            .iter()
            .filter(|item| item.catalog == catalog)
            .count()
        + merged
            .mcps
            .iter()
            .filter(|item| item.catalog == catalog)
            .count()
        + merged
            .hooks
            .iter()
            .filter(|item| item.catalog == catalog)
            .count()
}

/// Every catalog the config lists that no `requires` entry selects from, that has items, and that
/// the project did not publish itself.
///
/// This is the check that catches a typo'd `source:`. A catalog is a directory and nothing else, so
/// a misspelled path is no longer a parse failure: it is a directory holding none of the item
/// directories, i.e. a catalog with zero items. Where some pattern is qualified with that alias,
/// `unmatched-pattern` catches it instead and names the pattern the config wrote; this finding
/// covers what is left over, an alias nothing mentions at all. "Referenced" means qualified with,
/// not matched by, so reporting both an unmatched pattern and its catalog would count one mistake
/// twice.
///
/// Two exemptions, both following the same rule: what `ambit init` scaffolds is never a finding.
///
/// - A catalog with no items. The scaffolded `local` entry is live against three empty directories
///   while the `requires` entry that would select it is commented out, so a finding here would
///   fail `validate` on every freshly initialized project.
/// - The catalog the project is. This exemption stops applying once somebody puts a skill in
///   `skills/`, which is what a catalog repo is: a repo that publishes and consumes nothing. Every
///   item in it is checked regardless (this module's contract), and none of them being selected is
///   the normal state of a catalog repo, not a mistake.
///
/// Listed in `catalogs:` order (`merged.catalogs`' order), so the report reads down the file.
///
/// `own` is the catalogs whose root is the project directory itself, see [`ValidateOptions::own`].
fn unselected_catalog_problems(
    config: &ProjectConfig,
    merged: &MergedCatalog,
    own: &[String],
) -> Vec<ValidationProblem> {
    let mentioned: HashSet<&str> = config.requires.iter().map(entry_catalog).collect();

    merged
        .catalogs
        .iter()
        .filter(|catalog| !mentioned.contains(catalog.as_str()) && !own.contains(catalog))
        .map(|catalog| (catalog, item_count(merged, catalog)))
        .filter(|(_, items)| *items > 0)
        .map(|(catalog, items)| {
            problem(
                ValidationProblemKind::UnselectedCatalog,
                resolution_error(
                    format!(
                        "catalog \"{catalog}\" is configured but nothing selects from it {}",
                        at(&config.origin.file, None)
                    ),
                    [
                        format!(
                            "it provides {items} item{}, and no `{REQUIRES_KEY}` entry is qualified with \"{catalog}{CATALOG_SEPARATOR}\"",
                            if items == 1 { "" } else { "s" }
                        ),
                        "select what this project needs from it, or drop it from `catalogs:`"
                            .to_owned(),
                    ],
                ),
            )
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidateOptions {
    /// The project's config. Required: every report is about a project, because a catalog repo is
    /// one too, listing itself as `source: path:.` and validated through the same config as any
    /// other.
    pub config: ProjectConfig,
    /// Problems collected while parsing, listed ahead of the rest; see
    /// [`CatalogParseOptions`](crate::model::catalog::CatalogParseOptions).
    pub parsed: Vec<ValidationProblem>,
    /// The names of the catalogs the project is: the ones whose root resolved to the project
    /// directory itself, which is what `source: path:.` means and what `ambit init` scaffolds.
    ///
    /// A fact about where a source resolved to, so it cannot be recovered from the parsed config
    /// alone; passed in rather than worked out here. Read by `unselected_catalog_problems` to tell
    /// publishing from consuming. Empty means the project publishes nothing.
    pub own: Vec<String>,
}

/// Validates a merged catalog against the config that assembled it. Pure: it reads only the parsed
/// catalog and the parsed config, so [`validate_project`] touches the disk once and this decides
/// everything afterwards.
pub fn validate_catalog(merged: &MergedCatalog, options: &ValidateOptions) -> ValidationReport {
    let config = &options.config;
    let mut problems = options.parsed.clone();

    problems.extend(requirement_problems(merged));
    problems.extend(cycle_problems(merged));
    problems.extend(config_problems(config, merged));
    problems.extend(unselected_catalog_problems(config, merged, &options.own));

    ValidationReport {
        // Every copy, not every name: two catalogs providing `house-style` are two documents
        // checked here, and a count of one would understate what the report covers.
        checked: ValidationCounts {
            hooks: merged.hooks.len(),
            mcps: merged.mcps.len(),
            packs: merged.packs.len(),
            skills: merged.skills.len(),
        },
        problems,
    }
}

/// Validates everything a project configures: every catalog it lists, its own items among them,
/// and its own `requires` entries. The only entry point; a catalog repo runs this too, being a
/// project that lists itself.
///
/// Runs the same pipeline `resolve` does, minus resolution itself, so every finding here is a
/// finding `resolve` would raise. `context` is where sources resolve from, including `--offline`.
///
/// # Errors
///
/// Exit 2 for a missing or malformed config, an unresolvable source, or a catalog that does not
/// parse; exit 4 if a fetch fails.
pub fn validate_project(context: &SourceContext) -> Result<ValidationReport> {
    // Collects name↔path disagreements, the one problem parsing continues past.
    let mut collected: Vec<AmbitError> = Vec::new();

    let config = load_project_config(&context.project_dir)?;
    let catalogs = load_catalogs(
        &config,
        context,
        &mut CatalogLoadOptions {
            collect: Some(&mut collected),
            ..CatalogLoadOptions::default()
        },
    )?;
    // Which catalogs the project published rather than fetched: the directory a source resolved
    // to. `path:.` lands on the project root, and so does any other spelling of the same directory.
    let project_dir = normalize(&context.project_dir);
    let own: Vec<String> = catalogs
        .iter()
        .filter(|catalog| normalize(&catalog.root) == project_dir)
        .map(|catalog| catalog.name.clone())
        .collect();
    let parsed = collected
        .into_iter()
        .map(|found| problem(ValidationProblemKind::NameMismatch, found))
        .collect();

    Ok(validate_catalog(
        &merge_catalogs(&catalogs),
        &ValidateOptions {
            config,
            parsed,
            own,
        },
    ))
}

#[cfg(test)]
mod tests;
