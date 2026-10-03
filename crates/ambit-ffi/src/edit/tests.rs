use pretty_assertions::assert_eq;

use super::*;
use crate::records::SourceKind;

const FILE: &str = "ambit.yml";

const CONFIG: &str = "# Team setup.
version: 1
harnesses:
  - claude   # primary

catalogs:
  - name: company
    source: path:../company   # shared

requires:
  - skill: \"company/core.*\"   # everything core
";

fn address(kind: ItemKind, address: &str) -> EntryAddress {
    EntryAddress {
        kind,
        address: address.to_owned(),
    }
}

fn selection(kind: ItemKind, catalog: &str, pattern: &str) -> SelectionEntry {
    SelectionEntry {
        kind,
        catalog: Some(catalog.to_owned()),
        pattern: pattern.to_owned(),
        is_rule: pattern.contains('*'),
    }
}

#[test]
fn edits_keep_comments_and_return_the_new_summary() {
    let edited = edit_config(
        CONFIG,
        FILE,
        vec![
            ConfigEdit::AddEntry {
                entry: address(ItemKind::Mcp, "company/sentry"),
            },
            ConfigEdit::RenameCatalog {
                from: "company".to_owned(),
                to: "acme".to_owned(),
            },
        ],
    )
    .expect("the edits apply");

    assert_eq!(
        edited.text,
        "# Team setup.
version: 1
harnesses:
  - claude   # primary

catalogs:
  - name: acme
    source: path:../company   # shared

requires:
  - skill: \"acme/core.*\"   # everything core
  - mcp: \"acme/sentry\"
"
    );
    assert_eq!(edited.summary.catalogs[0].name, "acme");
    assert_eq!(
        edited.summary.requires,
        [
            selection(ItemKind::Skill, "acme", "core.*"),
            selection(ItemKind::Mcp, "acme", "sentry"),
        ]
    );
}

#[test]
fn replaces_and_removes_entries_by_address() {
    let edited = edit_config(
        CONFIG,
        FILE,
        vec![
            ConfigEdit::ReplaceEntry {
                old: address(ItemKind::Skill, "company/core.*"),
                new: address(ItemKind::Pack, "company/engineering"),
            },
            ConfigEdit::RemoveEntry {
                entry: address(ItemKind::Pack, "company/engineering"),
            },
            ConfigEdit::SetCatalogSource {
                name: "company".to_owned(),
                source: "acme/skills".to_owned(),
                git_ref: Some("v2".to_owned()),
            },
        ],
    )
    .expect("the edits apply");

    assert_eq!(edited.summary.requires, []);
    assert_eq!(edited.summary.catalogs[0].git_ref.as_deref(), Some("v2"));
}

#[test]
fn an_unqualified_address_is_refused_in_the_core_grammar() {
    let error = edit_config(
        CONFIG,
        FILE,
        vec![ConfigEdit::AddEntry {
            entry: address(ItemKind::Skill, "luma"),
        }],
    )
    .expect_err("an address needs a catalog");

    assert!(matches!(error, EngineError::Config { .. }), "{error:?}");
}

#[test]
fn an_invalid_config_reports_its_line() {
    let error = parse_config("version: 1\nextra: true\n", FILE).expect_err("unknown key");

    let EngineError::Config { path, line, .. } = error else {
        panic!("not a config error: {error:?}");
    };
    assert_eq!(path.as_deref(), Some(FILE));
    assert_eq!(line, Some(2));
}

#[test]
fn a_new_config_parses() {
    let text = new_config_text(vec!["cursor".to_owned(), "cursor".to_owned()]);
    let summary = parse_config(&text, "ambit.yaml").expect("a valid config");

    assert_eq!(summary.harnesses, ["cursor"]);
    assert_eq!(summary.catalogs, []);
}

#[test]
fn changes_against_no_base_are_all_additions() {
    let changes = config_changes(None, CONFIG, FILE).expect("valid configs");

    assert_eq!(changes.harnesses_added, ["claude"]);
    assert_eq!(
        changes.catalogs_added,
        [CatalogEntry {
            name: "company".to_owned(),
            source: "path:../company".to_owned(),
            git_ref: None,
            source_kind: Some(SourceKind::Local {
                path: "../company".to_owned()
            }),
        }]
    );
    assert_eq!(
        changes.entries_added,
        [selection(ItemKind::Skill, "company", "core.*")]
    );
    assert!(!changes.is_empty);
}

#[test]
fn a_rename_is_reported_as_one() {
    let draft = edit_config(
        CONFIG,
        FILE,
        vec![ConfigEdit::RenameCatalog {
            from: "company".to_owned(),
            to: "acme".to_owned(),
        }],
    )
    .expect("the edit applies");

    let changes = config_changes(Some(CONFIG.to_owned()), &draft.text, FILE).expect("valid");

    assert_eq!(
        changes.catalogs_renamed,
        [CatalogRename {
            from: "company".to_owned(),
            to: "acme".to_owned(),
        }]
    );
    assert_eq!(changes.entries_added, []);
    assert_eq!(changes.entries_removed, []);
    assert!(
        config_changes(Some(CONFIG.to_owned()), CONFIG, FILE)
            .expect("valid")
            .is_empty
    );
}

#[test]
fn lists_a_catalogs_references() {
    assert_eq!(
        catalog_references(CONFIG, FILE, "company").expect("valid"),
        [selection(ItemKind::Skill, "company", "core.*")]
    );
    assert_eq!(
        catalog_references(CONFIG, FILE, "other").expect("valid"),
        []
    );
}

#[test]
fn validates_catalog_names() {
    assert!(validate_catalog_name(CONFIG, FILE, "acme", None).is_ok());
    assert!(validate_catalog_name(CONFIG, FILE, "company", Some("company".to_owned())).is_ok());
    assert!(matches!(
        validate_catalog_name(CONFIG, FILE, "company", None),
        Err(EngineError::Config { .. })
    ));
    assert!(matches!(
        validate_catalog_name(CONFIG, FILE, "a/b", None),
        Err(EngineError::Config { .. })
    ));
}

#[test]
fn proposes_a_name_from_the_source() {
    assert_eq!(
        propose_catalog_name("git@github.com:acme/skills.git"),
        "skills"
    );
    assert_eq!(propose_catalog_name("path:../mine/"), "mine");
}
