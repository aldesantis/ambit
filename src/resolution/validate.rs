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
    pub enum ValidationProblemKind {
        Cycle => "cycle",
        NameMismatch => "name-mismatch",
        UnmatchedPattern => "unmatched-pattern",
        UnselectedCatalog => "unselected-catalog",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationProblem {
    pub kind: ValidationProblemKind,
    pub message: String,
    pub detail: Vec<String>,
}

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
    pub problems: Vec<ValidationProblem>,
}

pub fn is_valid(report: &ValidationReport) -> bool {
    report.problems.is_empty()
}

fn problem(kind: ValidationProblemKind, error: AmbitError) -> ValidationProblem {
    ValidationProblem {
        kind,
        message: error.message,
        detail: error.detail,
    }
}

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

fn node_key(kind: &str, catalog: &str, name: &str) -> String {
    format!("{kind}:{}", qualified_name(catalog, name))
}

fn requirer_key(requirer: &Requirer) -> String {
    node_key(requirer.kind.as_str(), &requirer.catalog, &requirer.name)
}

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
        let members: Vec<String> = cycle[..cycle.len() - 1].iter().map(requirer_key).collect();
        let start = members
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| js_cmp(a, b))
            .map_or(0, |(index, _)| index);
        // Escaped: a literal NUL byte makes grep treat this file as binary.
        let rotated = members[start..]
            .iter()
            .chain(&members[..start])
            .cloned()
            .collect::<Vec<_>>()
            .join("\u{0000}");

        if !self.reported.insert(rotated) {
            return;
        }

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
    pub config: ProjectConfig,
    pub parsed: Vec<ValidationProblem>,
    pub own: Vec<String>,
}

pub fn validate_catalog(merged: &MergedCatalog, options: &ValidateOptions) -> ValidationReport {
    let config = &options.config;
    let mut problems = options.parsed.clone();

    problems.extend(requirement_problems(merged));
    problems.extend(cycle_problems(merged));
    problems.extend(config_problems(config, merged));
    problems.extend(unselected_catalog_problems(config, merged, &options.own));

    ValidationReport {
        checked: ValidationCounts {
            hooks: merged.hooks.len(),
            mcps: merged.mcps.len(),
            packs: merged.packs.len(),
            skills: merged.skills.len(),
        },
        problems,
    }
}

pub fn validate_project(context: &SourceContext) -> Result<ValidationReport> {
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
