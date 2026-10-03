//! Every route to an item, and what uninstalling one takes, against the fixture catalog.
//!
//! The fixture's packs nest (`function.engineering.frontend` requires `function.engineering`, which
//! requires `core`), and `acme-brief` requires `company-context`, which `core` also gathers. So one
//! skill can be reached by a rule, an exact entry, a pack and a dependency at once.

use super::*;
use crate::errors::ExitCode;
use crate::model::catalog::{CatalogParseOptions, merge_catalogs, parse_catalog_directory};
use crate::model::config::parse_project_config;
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::tempdir;

const CATALOG: &str = "company";

/// The fixture catalog, merged on its own.
fn merged() -> MergedCatalog {
    let dir = tempdir();
    let root = build_fixture_catalog(&dir.path().join("catalog")).unwrap();
    let catalog = parse_catalog_directory(
        CATALOG,
        "path:catalog",
        &root,
        None,
        &mut CatalogParseOptions::default(),
    )
    .unwrap();

    merge_catalogs(&[catalog])
}

/// A config selecting `entries` (`kind`, qualified address) from the fixture catalog.
fn config(entries: &[(&str, &str)]) -> ProjectConfig {
    let list: String = entries
        .iter()
        .map(|(kind, address)| format!("\n  - {kind}: \"{address}\""))
        .collect::<Vec<_>>()
        .concat();
    let requires = if list.is_empty() {
        " []".to_owned()
    } else {
        list
    };

    parse_project_config(
        &format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG}\n    source: path:catalog\nrequires:{requires}\n"
        ),
        "ambit.yml",
    )
    .unwrap()
}

fn item(kind: ItemKind, name: &str) -> BundleItem {
    BundleItem {
        kind,
        name: name.to_owned(),
    }
}

fn entry(kind: ItemKind, pattern: &str) -> PatternEntry {
    PatternEntry {
        kind,
        pattern: pattern.to_owned(),
        catalog: Some(CATALOG.to_owned()),
    }
}

/// A route, rendered as `entry:<entry>` or `required-by:<item> via <chain>`.
fn shown(route: &Route) -> String {
    match route {
        Route::Entry { entry } => format!("entry:{}", format_entry(entry)),
        Route::RequiredBy { requirer, chain } => format!(
            "required-by:{}:{} via {}",
            requirer.kind,
            requirer.name,
            chain
                .iter()
                .map(|step| format!("{}:{}", step.kind, step.name))
                .collect::<Vec<_>>()
                .join(" > ")
        ),
    }
}

fn routes_for(routes: &[ItemRoutes], wanted: &BundleItem) -> Vec<String> {
    routes
        .iter()
        .find(|found| &found.item == wanted)
        .expect("the item is selected")
        .routes
        .iter()
        .map(shown)
        .collect()
}

fn names(items: &[BundleItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| format!("{}:{}", item.kind, item.name))
        .collect()
}

/// A rule, an exact entry, a pack and a skill's dependency, all reaching `company-context`.
fn four_routes() -> ProjectConfig {
    config(&[
        ("pack", "company/core"),
        ("pack", "company/project.acme"),
        ("skill", "company/company-context"),
        ("skill", "company/company-*"),
    ])
}

mod selection_routes {
    use super::*;

    #[test]
    fn lists_every_route_entries_first_then_packs_then_skills() {
        let merged = merged();
        let config = four_routes();
        let bundle = resolve_bundle(&config, &merged).unwrap();
        let routes = selection_routes(&config, &bundle).unwrap();

        assert_eq!(
            routes_for(&routes, &item(ItemKind::Skill, "company-context")),
            vec![
                "entry:skill:company/company-*",
                "entry:skill:company/company-context",
                "required-by:pack:core via pack:core > skill:company-context",
                "required-by:skill:acme-brief via pack:project.acme > skill:acme-brief > skill:company-context",
            ]
        );
    }

    #[test]
    fn walks_a_nested_pack_back_to_the_entry() {
        let merged = merged();
        let config = config(&[("pack", "company/function.engineering.frontend")]);
        let bundle = resolve_bundle(&config, &merged).unwrap();
        let routes = selection_routes(&config, &bundle).unwrap();

        assert_eq!(
            routes_for(&routes, &item(ItemKind::Skill, "company-context")),
            vec![
                "required-by:pack:core via pack:function.engineering.frontend > pack:function.engineering > pack:core > skill:company-context"
            ]
        );
        assert_eq!(
            routes_for(
                &routes,
                &item(ItemKind::Pack, "function.engineering.frontend")
            ),
            vec!["entry:pack:company/function.engineering.frontend"]
        );
    }

    #[test]
    fn covers_every_bundle_item_in_report_order() {
        let merged = merged();
        let config = config(&[("pack", "company/core")]);
        let bundle = resolve_bundle(&config, &merged).unwrap();
        let routes = selection_routes(&config, &bundle).unwrap();

        assert_eq!(
            routes
                .iter()
                .map(|found| format!("{}/{}:{}", found.catalog, found.item.kind, found.item.name))
                .collect::<Vec<_>>(),
            vec![
                "company/pack:core",
                "company/skill:company-context",
                "company/hook:session-notes",
            ]
        );
    }
}

mod unmatched {
    use super::*;

    #[test]
    fn reports_every_entry_that_matches_nothing_in_order() {
        let merged = merged();
        let config = config(&[
            ("skill", "company/zzz.*"),
            ("pack", "company/core"),
            ("skill", "elsewhere/anything"),
            ("mcp", "company/nope"),
        ]);
        let unmatched = unmatched_entries(&config, &merged);

        assert_eq!(
            unmatched
                .iter()
                .map(|(entry, _)| format_entry(entry))
                .collect::<Vec<_>>(),
            vec![
                "mcp:company/nope",
                "skill:company/zzz.*",
                "skill:elsewhere/anything"
            ]
        );
        assert!(
            unmatched
                .iter()
                .all(|(_, error)| error.code == ExitCode::Resolution)
        );
        assert!(unmatched[0].1.message.contains("(ambit.yml line 9)"));
        assert!(
            unmatched[2]
                .1
                .detail
                .iter()
                .any(|line| line.contains("no catalog in `catalogs:` is named \"elsewhere\""))
        );
    }

    #[test]
    fn are_ignored_by_the_rest_of_the_selection() {
        let merged = merged();
        let config = config(&[("pack", "company/core"), ("skill", "company/zzz.*")]);
        let bundle = resolve_matched(&config, &merged).unwrap();

        assert!(is_selected(
            &bundle,
            &item(ItemKind::Skill, "company-context")
        ));

        let impact =
            removal_impact(&config, &merged, &item(ItemKind::Skill, "company-context")).unwrap();

        assert_eq!(impact.sustaining.len(), 1);
    }

    #[test]
    fn is_empty_when_everything_matches() {
        assert_eq!(unmatched_entries(&four_routes(), &merged()), Vec::new());
    }
}

mod reach {
    use super::*;

    #[test]
    fn follows_the_closure_of_one_entry() {
        let merged = merged();
        let reach = entry_reach(&config(&[]), &merged, &entry(ItemKind::Pack, "core")).unwrap();

        assert_eq!(
            names(&reach),
            vec!["pack:core", "skill:company-context", "hook:session-notes"]
        );
    }

    #[test]
    fn refuses_an_entry_that_matches_nothing() {
        let error = entry_reach(&config(&[]), &merged(), &entry(ItemKind::Skill, "nope"))
            .expect_err("nothing matches");

        assert_eq!(error.code, ExitCode::Resolution);
    }
}

mod removal {
    use super::*;

    #[test]
    fn names_every_entry_keeping_a_shared_dependency() {
        let merged = merged();
        let impact = removal_impact(
            &four_routes(),
            &merged,
            &item(ItemKind::Skill, "company-context"),
        )
        .unwrap();

        assert_eq!(
            impact
                .sustaining
                .iter()
                .map(|sustained| format_entry(&sustained.entry))
                .collect::<Vec<_>>(),
            vec![
                "pack:company/core",
                "pack:company/project.acme",
                "skill:company/company-*",
                "skill:company/company-context",
            ]
        );
        assert_eq!(
            impact.sustaining[1]
                .chain
                .iter()
                .map(|step| format!("{}:{}", step.kind, step.name))
                .collect::<Vec<_>>(),
            vec![
                "pack:project.acme",
                "skill:acme-brief",
                "skill:company-context"
            ]
        );
    }

    #[test]
    fn never_claims_one_removal_uninstalls_what_another_route_keeps() {
        let merged = merged();
        let impact = removal_impact(
            &four_routes(),
            &merged,
            &item(ItemKind::Skill, "company-context"),
        )
        .unwrap();
        let effects: Vec<(String, Vec<String>)> = impact
            .effects
            .iter()
            .map(|(entry, dropped)| (format_entry(entry), names(dropped)))
            .collect();

        assert_eq!(
            effects,
            vec![
                (
                    "pack:company/core".to_owned(),
                    vec!["pack:core".to_owned(), "hook:session-notes".to_owned()]
                ),
                (
                    "pack:company/project.acme".to_owned(),
                    vec![
                        "pack:project.acme".to_owned(),
                        "skill:acme-brief".to_owned(),
                        "mcp:fixture".to_owned(),
                        "hook:acme-standup".to_owned(),
                    ]
                ),
                ("skill:company/company-*".to_owned(), vec![]),
                ("skill:company/company-context".to_owned(), vec![]),
            ]
        );
        assert_eq!(
            names(&impact.removed),
            vec![
                "pack:core",
                "pack:project.acme",
                "skill:acme-brief",
                "skill:company-context",
                "mcp:fixture",
                "hook:acme-standup",
                "hook:session-notes",
            ]
        );
    }

    #[test]
    fn names_the_pack_entry_for_a_pack_member() {
        let merged = merged();
        let config = config(&[
            ("pack", "company/function.engineering.frontend"),
            ("skill", "company/acme-brief"),
        ]);
        let impact =
            removal_impact(&config, &merged, &item(ItemKind::Skill, "code-review")).unwrap();

        assert_eq!(
            impact
                .sustaining
                .iter()
                .map(|sustained| format_entry(&sustained.entry))
                .collect::<Vec<_>>(),
            vec!["pack:company/function.engineering.frontend"]
        );
        // The dependency `acme-brief` shares with the pack survives the pack's removal.
        assert!(!names(&impact.removed).contains(&"skill:company-context".to_owned()));
        assert!(names(&impact.removed).contains(&"skill:code-review".to_owned()));
    }

    #[test]
    fn has_nothing_to_remove_for_an_unselected_item() {
        let merged = merged();
        let impact = removal_impact(
            &config(&[("pack", "company/core")]),
            &merged,
            &item(ItemKind::Skill, "code-review"),
        )
        .unwrap();

        assert_eq!(impact.sustaining, Vec::new());
        assert_eq!(impact.effects, Vec::new());
        assert_eq!(impact.removed, Vec::new());
    }
}
