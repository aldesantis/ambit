//! Config edits, asserted byte for byte: everything outside the edited nodes must come back
//! exactly as written.

use super::*;
use crate::errors::ExitCode;

const FILE: &str = "ambit.yml";

const CONFIG: &str = "# Team setup.
version: 1

# Tools we use.
harnesses:
  - claude   # primary
  - codex

catalogs:
  # The shared catalog.
  - name: company
    source: git@github.com:acme/skills.git
    ref: \"v1.2\"   # pinned

  - name: 'personal'
    source: path:../mine

requires:
  - pack: \"company/engineering\"   # everything
  - skill: company/core.*
  # Personal picks.
  - skill: 'personal/luma'
  - mcp: \"company/sentry\"
";

fn entry(kind: ItemKind, catalog: &str, pattern: &str) -> PatternEntry {
    PatternEntry {
        kind,
        pattern: pattern.to_owned(),
        catalog: Some(catalog.to_owned()),
    }
}

fn edit(text: &str, edits: &[ConfigEdit]) -> String {
    edit_config_text(text, FILE, edits).unwrap_or_else(|error| panic!("{}", error.format()))
}

fn refusal(text: &str, edits: &[ConfigEdit]) -> AmbitError {
    let error = edit_config_text(text, FILE, edits).expect_err("expected the edit to be refused");

    assert_eq!(error.code, ExitCode::Config, "{}", error.format());
    error
}

fn parse(text: &str) -> ProjectConfig {
    parse_project_config(text, FILE).unwrap()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|&value| value.to_owned()).collect()
}

/// `CONFIG` with `from` replaced by `to`, asserting `from` occurs exactly once.
fn changed(from: &str, to: &str) -> String {
    assert_eq!(CONFIG.matches(from).count(), 1, "{from:?}");
    CONFIG.replace(from, to)
}

#[test]
fn adds_a_selection_after_the_last_one() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::AddEntry {
            entry: entry(ItemKind::Hook, "personal", "guards.*"),
        }],
    );

    assert_eq!(edited, format!("{CONFIG}  - hook: \"personal/guards.*\"\n"));
}

#[test]
fn removes_a_selection_with_its_trailing_comment_and_keeps_other_comments() {
    let edited = edit(
        CONFIG,
        &[
            ConfigEdit::RemoveEntry {
                entry: entry(ItemKind::Pack, "company", "engineering"),
            },
            ConfigEdit::RemoveEntry {
                entry: entry(ItemKind::Skill, "personal", "luma"),
            },
        ],
    );

    assert_eq!(
        edited,
        changed(
            "  - pack: \"company/engineering\"   # everything\n  - skill: company/core.*\n  # Personal picks.\n  - skill: 'personal/luma'\n",
            "  - skill: company/core.*\n  # Personal picks.\n",
        )
    );
}

#[test]
fn removing_the_last_selection_leaves_an_empty_list() {
    let text = "version: 1\nrequires: # picks\n  - skill: \"c/a\"   # only one\n# end\n";
    let edited = edit(
        text,
        &[ConfigEdit::RemoveEntry {
            entry: entry(ItemKind::Skill, "c", "a"),
        }],
    );

    assert_eq!(edited, "version: 1\nrequires: [] # picks\n# end\n");
    assert_eq!(parse(&edited).requires, Vec::new());
}

#[test]
fn removes_every_copy_of_a_repeated_selection() {
    let text = "version: 1\nrequires:\n  - skill: c/a\n  - skill: c/b\n  - skill: \"c/a\"\n";
    let edited = edit(
        text,
        &[ConfigEdit::RemoveEntry {
            entry: entry(ItemKind::Skill, "c", "a"),
        }],
    );

    assert_eq!(edited, "version: 1\nrequires:\n  - skill: c/b\n");
}

#[test]
fn replaces_a_rule_pattern_keeping_its_quoting() {
    let edited = edit(
        CONFIG,
        &[
            ConfigEdit::ReplaceEntry {
                old: entry(ItemKind::Skill, "company", "core.*"),
                new: entry(ItemKind::Skill, "company", "core.lint.*"),
            },
            ConfigEdit::ReplaceEntry {
                old: entry(ItemKind::Skill, "personal", "luma"),
                new: entry(ItemKind::Skill, "personal", "lu*"),
            },
        ],
    );

    assert_eq!(
        edited,
        changed("company/core.*", "company/core.lint.*")
            .replace("'personal/luma'", "'personal/lu*'")
    );
}

#[test]
fn replacing_the_namespace_swaps_the_whole_item() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::ReplaceEntry {
            old: entry(ItemKind::Pack, "company", "engineering"),
            new: entry(ItemKind::Skill, "company", "engineering.*"),
        }],
    );

    assert_eq!(
        edited,
        changed(
            "- pack: \"company/engineering\"   # everything",
            "- skill: \"company/engineering.*\"   # everything",
        )
    );
}

#[test]
fn renames_a_catalog_and_rewrites_its_selections() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::RenameCatalog {
            from: "company".to_owned(),
            to: "acme".to_owned(),
        }],
    );

    assert_eq!(
        edited,
        CONFIG
            .replace("name: company", "name: acme")
            .replace("company/", "acme/")
    );

    let config = parse(&edited);

    assert_eq!(catalog_references(&config, "acme").len(), 3);
    assert_eq!(catalog_references(&config, "company"), Vec::new());
}

#[test]
fn renaming_to_a_name_needing_quotes_quotes_plain_values() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::RenameCatalog {
            from: "personal".to_owned(),
            to: "my: own".to_owned(),
        }],
    );

    assert_eq!(
        edited,
        CONFIG
            .replace("'personal'", "'my: own'")
            .replace("'personal/luma'", "'my: own/luma'")
    );
}

#[test]
fn removes_a_catalog_with_the_selections_qualified_with_it() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::RemoveCatalog {
            name: "company".to_owned(),
        }],
    );

    assert_eq!(
        edited,
        "# Team setup.
version: 1

# Tools we use.
harnesses:
  - claude   # primary
  - codex

catalogs:
  # The shared catalog.

  - name: 'personal'
    source: path:../mine

requires:
  # Personal picks.
  - skill: 'personal/luma'
"
    );
}

#[test]
fn removing_the_only_catalog_empties_both_lists() {
    let text =
        "version: 1\ncatalogs:\n  - name: c\n    source: path:.\nrequires:\n  - skill: c/a\n";
    let edited = edit(
        text,
        &[ConfigEdit::RemoveCatalog {
            name: "c".to_owned(),
        }],
    );

    assert_eq!(edited, "version: 1\ncatalogs: []\nrequires: []\n");
}

#[test]
fn changes_a_revision_keeping_its_quotes_and_comment() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::SetCatalogSource {
            name: "company".to_owned(),
            source: "git@github.com:acme/skills.git".to_owned(),
            r#ref: Some("v2".to_owned()),
        }],
    );

    assert_eq!(edited, changed("\"v1.2\"", "\"v2\""));
}

#[test]
fn removes_a_revision_line() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::SetCatalogSource {
            name: "company".to_owned(),
            source: "acme/skills".to_owned(),
            r#ref: None,
        }],
    );

    assert_eq!(
        edited,
        changed(
            "    source: git@github.com:acme/skills.git\n    ref: \"v1.2\"   # pinned\n",
            "    source: acme/skills\n",
        )
    );
}

#[test]
fn adds_a_revision_under_the_source() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::SetCatalogSource {
            name: "personal".to_owned(),
            source: "path:../mine".to_owned(),
            r#ref: Some("main".to_owned()),
        }],
    );

    assert_eq!(
        edited,
        changed(
            "    source: path:../mine\n",
            "    source: path:../mine\n    ref: main\n",
        )
    );
}

#[test]
fn a_revision_written_first_is_rewritten_with_its_item() {
    let text = "version: 1\ncatalogs:\n  - ref: v1 # old\n    name: c\n    source: a/b\n  - name: d\n    source: path:.\n";
    let edited = edit(
        text,
        &[ConfigEdit::SetCatalogSource {
            name: "c".to_owned(),
            source: "a/b".to_owned(),
            r#ref: None,
        }],
    );

    assert_eq!(
        edited,
        "version: 1\ncatalogs:\n  - name: c\n    source: a/b\n  - name: d\n    source: path:.\n"
    );
}

#[test]
fn adds_a_catalog_after_the_last_one() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::AddCatalog {
            name: "tools".to_owned(),
            source: "https://example.com/tools.git".to_owned(),
            r#ref: Some("1e5".to_owned()),
        }],
    );

    assert_eq!(
        edited,
        changed(
            "    source: path:../mine\n",
            "    source: path:../mine\n  - name: tools\n    source: https://example.com/tools.git\n    ref: \"1e5\"\n",
        )
    );
    assert_eq!(parse(&edited).catalogs[2].r#ref.as_deref(), Some("1e5"));
}

#[test]
fn sets_harnesses_by_removing_and_appending_items() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::SetHarnesses {
            harnesses: strings(&["claude", "cursor", "cursor"]),
        }],
    );

    assert_eq!(edited, changed("  - codex\n", "  - cursor\n"));
}

#[test]
fn reordering_harnesses_rewrites_the_list() {
    let edited = edit(
        CONFIG,
        &[ConfigEdit::SetHarnesses {
            harnesses: strings(&["codex", "claude"]),
        }],
    );

    assert_eq!(
        edited,
        changed(
            "  - claude   # primary\n  - codex\n",
            "  - codex\n  - claude\n"
        )
    );
}

#[test]
fn an_absent_harness_list_is_written_only_when_it_changes() {
    let text = "version: 1\n";

    assert_eq!(
        edit(
            text,
            &[ConfigEdit::SetHarnesses {
                harnesses: strings(&["claude"]),
            }]
        ),
        text
    );
    assert_eq!(
        edit(
            text,
            &[ConfigEdit::SetHarnesses {
                harnesses: strings(&["codex"]),
            }]
        ),
        "version: 1\nharnesses:\n  - codex\n"
    );
    assert_eq!(
        edit(
            text,
            &[ConfigEdit::SetHarnesses {
                harnesses: Vec::new(),
            }]
        ),
        "version: 1\nharnesses: []\n"
    );
}

#[test]
fn flow_lists_are_rewritten_in_block_style() {
    let text = "version: 1 # v\nharnesses: [claude] # tools\ncatalogs: [{ name: c, source: \"path:.\" }]\nrequires: [{ skill: \"c/a\" }, { hook: c/g }]  # picks\n";
    let edited = edit(
        text,
        &[
            ConfigEdit::SetHarnesses {
                harnesses: strings(&["claude", "codex"]),
            },
            ConfigEdit::AddEntry {
                entry: entry(ItemKind::Mcp, "c", "s"),
            },
        ],
    );

    assert_eq!(
        edited,
        "version: 1 # v\nharnesses:\n  - claude\n  - codex # tools\ncatalogs: [{ name: c, source: \"path:.\" }]\nrequires:\n  - skill: \"c/a\"\n  - hook: \"c/g\"\n  - mcp: \"c/s\"  # picks\n"
    );
}

#[test]
fn an_empty_flow_list_becomes_a_block_list() {
    let text = "version: 1\ncatalogs: []\nrequires: []\n";
    let edited = edit(
        text,
        &[
            ConfigEdit::AddCatalog {
                name: "c".to_owned(),
                source: "path:../c".to_owned(),
                r#ref: None,
            },
            ConfigEdit::AddEntry {
                entry: entry(ItemKind::Skill, "c", "a"),
            },
        ],
    );

    assert_eq!(
        edited,
        "version: 1\ncatalogs:\n  - name: c\n    source: path:../c\nrequires:\n  - skill: \"c/a\"\n"
    );
}

#[test]
fn a_flow_item_in_a_block_list_is_replaced_whole_when_patched() {
    let text = "version: 1\ncatalogs:\n  - { name: c, source: a/b }\n";
    let edited = edit(
        text,
        &[ConfigEdit::SetCatalogSource {
            name: "c".to_owned(),
            source: "a/b".to_owned(),
            r#ref: Some("v1".to_owned()),
        }],
    );

    assert_eq!(
        edited,
        "version: 1\ncatalogs:\n  - name: c\n    source: a/b\n    ref: v1\n"
    );
}

#[test]
fn missing_lists_are_appended_at_the_end() {
    let text = "version: 1 # no newline";
    let edited = edit(
        text,
        &[
            ConfigEdit::AddCatalog {
                name: "c".to_owned(),
                source: "path:.".to_owned(),
                r#ref: None,
            },
            ConfigEdit::AddEntry {
                entry: entry(ItemKind::Pack, "c", "all"),
            },
        ],
    );

    assert_eq!(
        edited,
        "version: 1 # no newline\ncatalogs:\n  - name: c\n    source: path:.\nrequires:\n  - pack: \"c/all\"\n"
    );
}

#[test]
fn keeps_the_dash_indentation_of_an_unindented_list() {
    let text = "version: 1\nrequires:\n- skill: c/a\n";
    let edited = edit(
        text,
        &[ConfigEdit::AddEntry {
            entry: entry(ItemKind::Skill, "c", "b"),
        }],
    );

    assert_eq!(
        edited,
        "version: 1\nrequires:\n- skill: c/a\n- skill: \"c/b\"\n"
    );
}

#[test]
fn keeps_crlf_line_endings() {
    let text = "version: 1\r\nrequires:\r\n  - skill: c/a\r\n";
    let edited = edit(
        text,
        &[ConfigEdit::AddEntry {
            entry: entry(ItemKind::Skill, "c", "b"),
        }],
    );

    assert_eq!(
        edited,
        "version: 1\r\nrequires:\r\n  - skill: c/a\r\n  - skill: \"c/b\"\r\n"
    );
}

#[test]
fn edits_an_ambit_yaml_file_the_same_way() {
    let edited = edit_config_text(
        CONFIG,
        "ambit.yaml",
        &[ConfigEdit::AddEntry {
            entry: entry(ItemKind::Skill, "personal", "x"),
        }],
    )
    .unwrap();

    assert_eq!(edited, format!("{CONFIG}  - skill: \"personal/x\"\n"));

    let error = edit_config_text(
        "version: 2\n",
        "ambit.yaml",
        &[ConfigEdit::RemoveCatalog {
            name: "c".to_owned(),
        }],
    )
    .unwrap_err();

    assert!(
        error.message.contains("(ambit.yaml line 1)"),
        "{}",
        error.message
    );
}

#[test]
fn adding_then_removing_restores_the_original_bytes() {
    let added = entry(ItemKind::Skill, "company", "extra.*");
    let edited = edit(
        CONFIG,
        &[
            ConfigEdit::AddCatalog {
                name: "tools".to_owned(),
                source: "path:../tools".to_owned(),
                r#ref: None,
            },
            ConfigEdit::AddEntry {
                entry: added.clone(),
            },
            ConfigEdit::RemoveEntry { entry: added },
            ConfigEdit::RemoveCatalog {
                name: "tools".to_owned(),
            },
        ],
    );

    assert_eq!(edited, CONFIG);
}

#[test]
fn renaming_there_and_back_restores_the_original_bytes() {
    let edited = edit(
        CONFIG,
        &[
            ConfigEdit::RenameCatalog {
                from: "company".to_owned(),
                to: "x".to_owned(),
            },
            ConfigEdit::RenameCatalog {
                from: "x".to_owned(),
                to: "company".to_owned(),
            },
        ],
    );

    assert_eq!(edited, CONFIG);
}

#[test]
fn no_edits_change_nothing() {
    assert_eq!(edit(CONFIG, &[]), CONFIG);
}

#[test]
fn refuses_a_malformed_config_naming_the_line() {
    let error = refusal(
        "version: 1\nrequires:\n  - \"c/a\"\n",
        &[ConfigEdit::RemoveCatalog {
            name: "c".to_owned(),
        }],
    );

    assert!(
        error.message.contains("(ambit.yml line 3)"),
        "{}",
        error.message
    );
}

#[test]
fn refuses_to_edit_an_anchored_or_tagged_list() {
    let anchored = "version: 1\nrequires: &picks\n  - skill: c/a\n";
    let error = refusal(
        anchored,
        &[ConfigEdit::AddEntry {
            entry: entry(ItemKind::Skill, "c", "b"),
        }],
    );

    assert_eq!(
        error.message,
        "cannot edit `requires`: it uses an anchor (ambit.yml line 3)"
    );

    let tagged = "version: 1\nharnesses: !!seq [claude]\n";
    let error = refusal(
        tagged,
        &[ConfigEdit::SetHarnesses {
            harnesses: strings(&["codex"]),
        }],
    );

    assert!(error.message.contains("uses a tag"), "{}", error.message);

    // An anchor elsewhere in the document does not stop an edit that leaves it alone.
    let elsewhere = "version: 1\nharnesses: &h [claude]\nrequires:\n  - skill: c/a\n";

    assert_eq!(
        edit(
            elsewhere,
            &[ConfigEdit::AddEntry {
                entry: entry(ItemKind::Skill, "c", "b"),
            }]
        ),
        format!("{elsewhere}  - skill: \"c/b\"\n")
    );
}

#[test]
fn refuses_a_flow_root_and_a_document_end_marker() {
    let error = refusal(
        "{version: 1}\n",
        &[ConfigEdit::AddEntry {
            entry: entry(ItemKind::Skill, "c", "b"),
        }],
    );

    assert!(error.message.contains("flow mapping"), "{}", error.message);

    let error = refusal(
        "version: 1\n...\n",
        &[ConfigEdit::AddEntry {
            entry: entry(ItemKind::Skill, "c", "b"),
        }],
    );

    assert!(error.message.contains("end marker"), "{}", error.message);
}

#[test]
fn refuses_edits_naming_what_the_config_lacks_or_already_has() {
    let unknown = refusal(
        CONFIG,
        &[ConfigEdit::RemoveCatalog {
            name: "nope".to_owned(),
        }],
    );

    assert_eq!(unknown.message, "no catalog named \"nope\" (ambit.yml)");

    let missing = refusal(
        CONFIG,
        &[ConfigEdit::RemoveEntry {
            entry: entry(ItemKind::Skill, "company", "nope"),
        }],
    );

    assert_eq!(
        missing.message,
        "`skill:company/nope` is not selected (ambit.yml)"
    );

    let duplicate = refusal(
        CONFIG,
        &[ConfigEdit::AddEntry {
            entry: entry(ItemKind::Mcp, "company", "sentry"),
        }],
    );

    assert_eq!(
        duplicate.message,
        "`mcp:company/sentry` is already selected (ambit.yml)"
    );

    let taken = refusal(
        CONFIG,
        &[ConfigEdit::RenameCatalog {
            from: "company".to_owned(),
            to: "personal".to_owned(),
        }],
    );

    assert_eq!(
        taken.message,
        "duplicate catalog name \"personal\" (ambit.yml)"
    );
}

#[test]
fn refuses_invalid_values() {
    let slash = refusal(
        CONFIG,
        &[ConfigEdit::AddCatalog {
            name: "a/b".to_owned(),
            source: "path:.".to_owned(),
            r#ref: None,
        }],
    );

    assert!(slash.message.contains("holds a `/`"), "{}", slash.message);

    let newline = refusal(
        CONFIG,
        &[ConfigEdit::AddCatalog {
            name: "x".to_owned(),
            source: "path:.\nversion: 2".to_owned(),
            r#ref: None,
        }],
    );

    assert!(
        newline.message.contains("control character"),
        "{}",
        newline.message
    );

    let unqualified = refusal(
        CONFIG,
        &[ConfigEdit::AddEntry {
            entry: PatternEntry {
                kind: ItemKind::Skill,
                pattern: "a".to_owned(),
                catalog: None,
            },
        }],
    );

    assert!(
        unqualified.message.contains("names no catalog"),
        "{}",
        unqualified.message
    );

    let empty = refusal(
        CONFIG,
        &[ConfigEdit::AddEntry {
            entry: entry(ItemKind::Skill, "company", " "),
        }],
    );

    assert_eq!(empty.message, "selection pattern must not be empty");
}

#[test]
fn values_with_quotes_round_trip() {
    let pattern = "it's \"odd\"";
    let edited = edit(
        CONFIG,
        &[ConfigEdit::ReplaceEntry {
            old: entry(ItemKind::Skill, "personal", "luma"),
            new: entry(ItemKind::Skill, "personal", pattern),
        }],
    );

    assert_eq!(
        edited,
        changed("'personal/luma'", "'personal/it''s \"odd\"'")
    );
    assert!(
        parse(&edited)
            .requires
            .contains(&entry(ItemKind::Skill, "personal", pattern))
    );
}

#[test]
fn new_config_text_parses_to_an_empty_setup() {
    let text = new_config_text(&strings(&["claude", "codex"]));

    assert_eq!(
        text,
        "version: 1\nharnesses:\n  - claude\n  - codex\ncatalogs: []\nrequires: []\n"
    );

    let config = parse(&text);

    assert_eq!(config.harnesses, strings(&["claude", "codex"]));
    assert!(config.catalogs.is_empty() && config.requires.is_empty());
    assert_eq!(parse(&new_config_text(&[])).harnesses, Vec::<String>::new());
}

#[test]
fn validates_catalog_names() {
    let config = parse(CONFIG);

    assert!(validate_catalog_name(&config, "tools", None).is_ok());
    assert!(validate_catalog_name(&config, "company", Some("company")).is_ok());
    assert!(validate_catalog_name(&config, "company", None).is_err());
    assert!(validate_catalog_name(&config, "", None).is_err());
    assert!(validate_catalog_name(&config, "a/b", None).is_err());
    assert!(validate_catalog_name(&config, "a.b*", None).is_ok());
}

#[test]
fn proposes_a_catalog_name_from_a_source() {
    for (source, name) in [
        ("git@github.com:acme/skills.git", "skills"),
        ("https://github.com/acme/tools.git/", "tools"),
        ("acme/house-style@v2", "house-style"),
        ("path:../my-catalog/", "my-catalog"),
        ("path:.", "catalog"),
        ("git:ssh://git@host/repo", "repo"),
    ] {
        assert_eq!(propose_catalog_name(source), name, "{source}");
    }
}

#[test]
fn diffs_two_configs() {
    let base = parse(CONFIG);
    let draft = parse(&edit(
        CONFIG,
        &[
            ConfigEdit::RenameCatalog {
                from: "company".to_owned(),
                to: "acme".to_owned(),
            },
            ConfigEdit::SetCatalogSource {
                name: "personal".to_owned(),
                source: "path:../other".to_owned(),
                r#ref: None,
            },
            ConfigEdit::RemoveEntry {
                entry: entry(ItemKind::Mcp, "acme", "sentry"),
            },
            ConfigEdit::AddEntry {
                entry: entry(ItemKind::Hook, "personal", "*"),
            },
            ConfigEdit::SetHarnesses {
                harnesses: strings(&["claude", "cursor"]),
            },
        ],
    ));
    let changes = config_changes(Some(&base), &draft);

    assert_eq!(
        changes,
        ConfigChanges {
            harnesses_added: strings(&["cursor"]),
            harnesses_removed: strings(&["codex"]),
            catalogs_added: Vec::new(),
            catalogs_removed: Vec::new(),
            catalogs_changed: vec![(base.catalogs[1].clone(), draft.catalogs[1].clone())],
            catalogs_renamed: vec![("company".to_owned(), "acme".to_owned())],
            entries_added: vec![entry(ItemKind::Hook, "personal", "*")],
            entries_removed: vec![entry(ItemKind::Mcp, "company", "sentry")],
        }
    );
    assert!(config_changes(Some(&base), &base).is_empty());

    let created = config_changes(None, &base);

    assert_eq!(created.catalogs_added, base.catalogs);
    assert_eq!(created.entries_added, base.requires);
    assert_eq!(created.harnesses_added, base.harnesses);
}
