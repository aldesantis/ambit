//! `ambit.yml` parsing, on top of the shared loader.
//!
//! The malformed cases assert the [`AmbitError`](crate::errors::AmbitError) code the CLI turns into
//! an exit status: exit 2 for every config problem.

use regex::Regex;

use super::*;
use crate::errors::{AmbitError, ExitCode};
use crate::test_support::tempdir;
use crate::util::fs::write_text;

const FILE: &str = "ambit.yml";

/// One `requires` entry, for the cases about which file was read rather than about what it said.
const ONE_ENTRY: &str = "requires: [{ skill: \"c/core\" }]\n";

fn entry(kind: ItemKind, pattern: &str, catalog: &str) -> PatternEntry {
    PatternEntry {
        kind,
        pattern: pattern.to_owned(),
        catalog: Some(catalog.to_owned()),
    }
}

fn one_entry_parsed() -> PatternEntry {
    entry(ItemKind::Skill, "core", "c")
}

fn lines(pairs: &[(&str, usize)]) -> IndexMap<String, usize> {
    pairs
        .iter()
        .map(|&(key, line)| (key.to_owned(), line))
        .collect()
}

fn parse(text: &str) -> ProjectConfig {
    parse_project_config(text, FILE).unwrap()
}

/// Parses `text`, asserting it was rejected as a config error (exit 2).
fn rejection(text: &str) -> AmbitError {
    let error = parse_project_config(text, FILE).expect_err("expected the config to be rejected");

    assert_eq!(error.code, ExitCode::Config, "{}", error.format());
    error
}

const FULL_CONFIG: &str = "version: 1
harnesses: [claude]

requires:
  - pack: \"company/function.engineering\"
  - skill: \"company/core.*\"
  - skill: \"personal/luma\"

catalogs:
  - name: company
    source: git@github.com:acme/skills.git
    ref: \"a1b2c3d4\"
  - name: personal
    source: git@github.com:jane/skills-private.git
    ref: main
";

#[test]
fn parses_the_specs_own_example_into_a_typed_object() {
    assert_eq!(
        parse(FULL_CONFIG),
        ProjectConfig {
            version: 1,
            origin: ConfigOrigin {
                file: FILE.to_owned(),
                entry_lines: lines(&[
                    ("- pack: \"company/function.engineering\"", 5),
                    ("- skill: \"company/core.*\"", 6),
                    ("- skill: \"personal/luma\"", 7),
                ]),
            },
            harnesses: vec!["claude".to_owned()],
            catalogs: vec![
                CatalogRef {
                    name: "company".to_owned(),
                    source: "git@github.com:acme/skills.git".to_owned(),
                    r#ref: Some("a1b2c3d4".to_owned()),
                    path: None,
                    trust: Trust::Review,
                },
                CatalogRef {
                    name: "personal".to_owned(),
                    source: "git@github.com:jane/skills-private.git".to_owned(),
                    r#ref: Some("main".to_owned()),
                    path: None,
                    trust: Trust::Review,
                },
            ],
            requires: vec![
                entry(ItemKind::Pack, "function.engineering", "company"),
                entry(ItemKind::Skill, "core.*", "company"),
                entry(ItemKind::Skill, "luma", "personal"),
            ],
        }
    );
}

#[test]
fn defaults_everything_but_the_version() {
    assert_eq!(
        parse("version: 1\n"),
        ProjectConfig {
            version: 1,
            origin: ConfigOrigin {
                file: FILE.to_owned(),
                entry_lines: IndexMap::new(),
            },
            harnesses: DEFAULT_HARNESSES.iter().map(|&h| h.to_owned()).collect(),
            catalogs: vec![],
            requires: vec![],
        }
    );
}

#[test]
fn records_the_line_each_requires_entry_was_written_on() {
    // Resolution rejects an entry that matches nothing long after this parse, and the error is
    // still expected to name the line, so the positions have to survive parsing.
    let config = parse(&["version: 1", "requires:", "  - { skill: c/core }", ""].join("\n"));

    assert_eq!(
        config.origin,
        ConfigOrigin {
            file: FILE.to_owned(),
            entry_lines: lines(&[("- skill: \"c/core\"", 3)]),
        }
    );
}

#[test]
fn keys_the_lines_by_the_whole_entry_so_two_namespaces_keep_both() {
    let config = parse(
        &[
            "version: 1",
            "requires:",
            "  - { skill: c/core }",
            "  - { hook: c/core }",
            "",
        ]
        .join("\n"),
    );

    assert_eq!(
        config.origin.entry_lines,
        lines(&[("- skill: \"c/core\"", 3), ("- hook: \"c/core\"", 4)])
    );
}

#[test]
fn keeps_the_first_line_of_an_entry_written_twice_and_the_entry_once() {
    let config = parse(
        &[
            "version: 1",
            "requires:",
            "  - { skill: c/core }",
            "  - { skill: c/core }",
            "",
        ]
        .join("\n"),
    );

    assert_eq!(config.requires.len(), 1);
    assert_eq!(
        config.origin.entry_lines.get("- skill: \"c/core\""),
        Some(&3)
    );
}

#[test]
fn keeps_the_entries_exactly_as_listed_adding_nothing() {
    // Nothing is implicit. A config that selects one thing selects one thing.
    let config = parse("version: 1\nrequires: [{ pack: \"c/function.sales\" }]\n");

    assert_eq!(
        config.requires,
        vec![entry(ItemKind::Pack, "function.sales", "c")]
    );
}

#[test]
fn reads_an_empty_requires_list_as_selecting_nothing() {
    assert_eq!(
        parse("version: 1\nrequires: []\n").requires,
        Vec::<PatternEntry>::new()
    );
}

#[test]
fn omits_an_absent_catalog_ref_rather_than_inventing_one() {
    let config = parse("version: 1\ncatalogs:\n  - name: company\n    source: acme/skills\n");

    assert_eq!(
        config.catalogs[0],
        CatalogRef {
            name: "company".to_owned(),
            source: "acme/skills".to_owned(),
            r#ref: None,
            path: None,
            trust: Trust::Review,
        }
    );
}

#[test]
fn reviews_a_fetched_catalog_and_trusts_a_path_one_unless_told_otherwise() {
    let config = parse(
        "version: 1\ncatalogs:\n  - name: a\n    source: acme/skills\n  - name: b\n    source: git:ssh://host/x.git\n  - name: c\n    source: path:../c\n  - name: d\n    source: acme/skills\n    trust: full\n  - name: e\n    source: path:.\n    trust: review\n",
    );
    let trust: Vec<Trust> = config
        .catalogs
        .iter()
        .map(|catalog| catalog.trust)
        .collect();

    assert_eq!(
        trust,
        [
            Trust::Review,
            Trust::Review,
            Trust::Full,
            Trust::Full,
            Trust::Review
        ]
    );
}

#[test]
fn keeps_catalogs_in_config_order() {
    let config = parse(
        "version: 1\ncatalogs:\n  - name: b\n    source: x/b\n  - name: a\n    source: x/a\n",
    );

    assert_eq!(
        config
            .catalogs
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["b", "a"]
    );
}

mod rejections {
    use super::*;

    #[test]
    fn rejects_an_unknown_top_level_key() {
        let error = rejection("version: 1\nrequire:\n  - core\n");

        assert!(
            error
                .format()
                .contains(&format!("unknown key \"require\" ({FILE} line 2)"))
        );
        assert!(
            error
                .format()
                .contains("accepted keys: catalogs, harnesses, requires, version")
        );
    }

    #[test]
    fn requires_a_version() {
        assert!(
            rejection("harnesses: [claude]\n")
                .format()
                .contains("missing required key \"version\"")
        );
    }

    #[test]
    fn rejects_a_version_it_does_not_understand() {
        let error = rejection("version: 2\n");

        assert!(
            error
                .format()
                .contains(&format!("unsupported config version 2 ({FILE} line 1)"))
        );
        assert!(
            error
                .format()
                .contains("set `version: 1`, or upgrade ambit")
        );
    }

    #[test]
    fn rejects_a_numeric_ref_rather_than_stringifying_it() {
        let error = rejection(
            "version: 1\ncatalogs:\n  - name: company\n    source: acme/skills\n    ref: 1234567\n",
        );

        assert!(error.format().contains(&format!(
            "\"catalogs[0].ref\" must be a string ({FILE} line 5)"
        )));
        assert!(error.format().contains("quote it: `ref: \"1234567\"`"));
    }

    #[test]
    fn rejects_two_catalogs_with_the_same_name_naming_both_lines() {
        let error = rejection(
            "version: 1\ncatalogs:\n  - name: c\n    source: a/b\n  - name: c\n    source: c/d\n",
        );

        assert!(
            error
                .format()
                .contains(&format!("duplicate catalog name \"c\" ({FILE} line 5)"))
        );
        assert!(error.format().contains("first declared on line 3"));
    }

    #[test]
    fn rejects_a_catalog_name_holding_the_address_separator() {
        // An alias is the qualifier half of `<catalog>/<pattern>`. One holding a `/` is addressable
        // by nothing, so it is refused where it is written.
        let error = rejection("version: 1\ncatalogs:\n  - name: a/b\n    source: x/y\n");

        assert!(
            error
                .format()
                .contains(&format!("catalog name \"a/b\" holds a `/` ({FILE} line 3)"))
        );
        assert!(
            error
                .format()
                .contains("rename the catalog to something without a `/`")
        );
    }

    #[test]
    fn accepts_a_catalog_name_holding_a_dot() {
        let config = parse("version: 1\ncatalogs:\n  - name: acme.company\n    source: x/y\n");

        assert_eq!(config.catalogs[0].name, "acme.company");
    }

    #[test]
    fn rejects_an_unknown_key_inside_a_catalog_entry() {
        assert!(
            rejection("version: 1\ncatalogs:\n  - name: c\n    source: a/b\n    branch: main\n")
                .format()
                .contains("unknown key \"catalogs[0].branch\"")
        );
    }

    #[test]
    fn normalizes_a_catalog_path() {
        let config = parse(
            "version: 1\ncatalogs:\n  - name: c\n    source: a/b\n    path: ./plugins//acme/\n",
        );

        assert_eq!(config.catalogs[0].path.as_deref(), Some("plugins/acme"));
    }

    #[test]
    fn reads_a_catalog_path_naming_the_root_as_no_path() {
        let config = parse("version: 1\ncatalogs:\n  - name: c\n    source: a/b\n    path: .\n");

        assert_eq!(config.catalogs[0].path, None);
    }

    #[test]
    fn rejects_a_catalog_path_that_leaves_its_source() {
        let error =
            rejection("version: 1\ncatalogs:\n  - name: c\n    source: a/b\n    path: x/../../y\n");

        assert!(error.format().contains(&format!(
            "catalog path \"x/../../y\" leaves its source ({FILE} line 5)"
        )));
    }

    #[test]
    fn rejects_a_trust_it_does_not_know_naming_the_ones_it_does() {
        let error =
            rejection("version: 1\ncatalogs:\n  - name: c\n    source: a/b\n    trust: some\n");

        assert!(
            error
                .format()
                .contains(&format!("unknown trust \"some\" ({FILE} line 5)")),
            "{}",
            error.format()
        );
        assert!(error.format().contains("`trust` is one of: full, review"));
    }

    #[test]
    fn rejects_an_absolute_catalog_path() {
        assert!(
            rejection("version: 1\ncatalogs:\n  - name: c\n    source: a/b\n    path: /plugins\n")
                .format()
                .contains("catalog path \"/plugins\" is absolute")
        );
    }

    #[test]
    fn rejects_a_requires_entry_through_the_shared_grammar() {
        // The grammar's own refusals are `pattern`'s tests; this is the claim that a project config
        // reads its `requires` through them rather than through a second, looser parser.
        assert!(
            rejection("version: 1\nrequires: [{ tag: \"c/core\" }]\n")
                .format()
                .contains("accepted keys: hook, mcp, pack, skill")
        );
    }

    #[test]
    fn rejects_a_requires_address_that_names_no_catalog() {
        let error = rejection("version: 1\nrequires: [{ skill: \"core\" }]\n");

        assert!(
            error
                .format()
                .contains("`requires` entry \"core\" names no catalog")
        );
        assert!(error.format().contains("qualify it: `<catalog>/core`"));
    }
}

/// The forms that used to put a definition in `ambit.yml` itself.
///
/// The refusal is the migration path, so each message carries both halves of the move: the file
/// the definition goes into, and the `catalogs:` entry that makes the file reachable.
mod inline_definitions_refused {
    use super::*;

    #[test]
    fn refuses_a_top_level_mcps_naming_the_file_and_the_catalog_entry() {
        let error = rejection(
            "version: 1\nmcps:\n  - name: x\n    transport:\n      stdio:\n        command: npx\n",
        );

        assert!(
            error
                .format()
                .contains(&format!("top-level `mcps` is gone ({FILE} line 2)"))
        );
        assert!(error.format().contains(
            "an MCP server is defined by a file of its own: move each entry to `mcps/<name>.yml`"
        ));
        assert!(error.format().contains(
            "then list this project as a catalog: `- name: local` with `source: path:.`"
        ));
    }

    #[test]
    fn refuses_a_top_level_hooks_naming_the_file_and_the_catalog_entry() {
        let error = rejection(
            "version: 1\nhooks:\n  - name: x\n    event: Stop\n    type: command\n    command: x.sh\n",
        );

        assert!(
            error
                .format()
                .contains(&format!("top-level `hooks` is gone ({FILE} line 2)"))
        );
        assert!(error.format().contains(
            "a hook is defined by a file of its own: move each entry to `hooks/<name>/hook.yml`"
        ));
        assert!(error.format().contains(
            "then list this project as a catalog: `- name: local` with `source: path:.`"
        ));
    }

    #[test]
    fn refuses_them_ahead_of_the_unknown_key_check() {
        assert!(
            !rejection("version: 1\nmcps: []\n")
                .format()
                .contains("unknown key")
        );
    }
}

/// The two keys a project used to select with.
///
/// The refusal is the migration path, so it carries the rewrite per line. The catalog alias is in
/// the same file, so the printed entry is the one the reader can paste.
mod the_deleted_selection_keys_refused {
    use super::*;

    const CATALOGS: &str = "catalogs:\n  - name: company\n    source: acme/skills\n";

    #[test]
    fn refuses_a_top_level_scopes_naming_what_does_the_job_now() {
        let error = rejection(&format!(
            "version: 1\n{CATALOGS}scopes:\n  - core\n  - function.engineering\n"
        ));

        assert!(
            error
                .format()
                .contains(&format!("top-level `scopes` is gone ({FILE} line 5)"))
        );
        // A scope reached across every namespace at once, which one entry does not.
        assert!(error.format().contains(
            "declare a pack in the catalog that requires them, and select it with `pack:`"
        ));
        assert!(error.format().contains("rename the key to `requires`"));
    }

    #[test]
    fn refuses_a_top_level_skills_naming_the_entry_each_becomes() {
        let error = rejection(&format!("version: 1\n{CATALOGS}skills:\n  - house-style\n"));

        assert!(
            error
                .format()
                .contains(&format!("top-level `skills` is gone ({FILE} line 5)"))
        );
        assert!(
            error
                .format()
                .contains("line 6: `house-style` becomes `- skill: \"company/house-style\"`")
        );
        // A `skill:` entry names one namespace outright, so nothing about a pack is suggested.
        assert!(!error.format().contains("declare a pack"));
    }

    #[test]
    fn declines_to_pick_an_alias_when_the_config_declares_more_than_one_catalog() {
        let error = rejection(
            &[
                "version: 1",
                "catalogs:",
                "  - name: company",
                "    source: acme/skills",
                "  - name: personal",
                "    source: jane/skills",
                "scopes: [core]",
                "",
            ]
            .join("\n"),
        );

        assert!(
            error
                .format()
                .contains("qualifying each entry with the alias it should select from")
        );
    }

    #[test]
    fn still_names_the_rewrite_when_catalogs_is_malformed() {
        // The alias is a courtesy in this message; a broken `catalogs:` is refused on its own terms
        // once the removed key is gone.
        assert!(
            rejection("version: 1\ncatalogs: nope\nscopes: [core]\n")
                .format()
                .contains("top-level `scopes` is gone")
        );
    }

    #[test]
    fn refuses_them_ahead_of_the_unknown_key_check() {
        assert!(
            !rejection("version: 1\nscopes: []\n")
                .format()
                .contains("unknown key")
        );
        assert!(
            !rejection("version: 1\nskills: []\n")
                .format()
                .contains("unknown key")
        );
    }
}

/// The loader's rules reach `ambit.yml` through the shared loader. Asserted here too, because a
/// config is the document a person hand-writes and so the one these mistakes land in.
#[test]
fn yaml_rules_as_seen_from_a_config_exit_2_naming_the_problem_and_its_line() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "a duplicate key",
            "version: 1\nrequires: [a]\nrequires: [b]\n",
            r#"duplicate key "requires" \(ambit\.yml line 3\)"#,
        ),
        (
            "tab indentation",
            "version: 1\nharnesses:\n\t- claude\n",
            r"tabs for indentation \(ambit\.yml line 3\)",
        ),
        (
            "a custom tag",
            "version: 1\nrequires: !!python/object []\n",
            r"custom YAML tag .* \(ambit\.yml line 2\)",
        ),
        ("an empty document", "", r"ambit\.yml is empty"),
        (
            "a non-mapping root",
            "- version: 1\n",
            r"root is not a mapping \(ambit\.yml line 1\)",
        ),
        (
            "an unknown key",
            "version: 1\nharness: claude\n",
            r#"unknown key "harness" \(ambit\.yml line 2\)"#,
        ),
        (
            "an explicit null",
            "version: null\n",
            r#""version" must not be null \(ambit\.yml line 1\)"#,
        ),
        (
            "a ref that parsed as a number",
            "version: 1\ncatalogs:\n  - name: c\n    source: a/b\n    ref: 1234567\n",
            r#""catalogs\[0\]\.ref" must be a string \(ambit\.yml line 5\)"#,
        ),
    ];

    for &(label, text, expected) in cases {
        let formatted = rejection(text).format();

        assert!(
            Regex::new(expected).unwrap().is_match(&formatted),
            "{label}: {formatted}"
        );
    }
}

mod config_discovery {
    use super::*;

    #[test]
    fn loads_ambit_yml() {
        let dir = tempdir();

        write_text(
            &dir.path().join("ambit.yml"),
            &format!("version: 1\n{ONE_ENTRY}"),
        )
        .unwrap();

        assert_eq!(
            find_config_file(dir.path()).unwrap(),
            FoundConfig {
                path: dir.path().join("ambit.yml"),
                file: "ambit.yml".to_owned(),
            }
        );
        assert_eq!(
            load_project_config(dir.path()).unwrap().requires,
            vec![one_entry_parsed()]
        );
    }

    #[test]
    fn accepts_ambit_yaml() {
        let dir = tempdir();

        write_text(
            &dir.path().join("ambit.yaml"),
            &format!("version: 1\n{ONE_ENTRY}"),
        )
        .unwrap();

        assert_eq!(find_config_file(dir.path()).unwrap().file, "ambit.yaml");
        assert_eq!(
            load_project_config(dir.path()).unwrap().requires,
            vec![one_entry_parsed()]
        );
    }

    #[test]
    fn refuses_to_guess_when_both_exist() {
        let dir = tempdir();

        write_text(&dir.path().join("ambit.yml"), "version: 1\n").unwrap();
        write_text(&dir.path().join("ambit.yaml"), "version: 1\n").unwrap();

        let error = load_project_config(dir.path()).unwrap_err();

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(
            error.message,
            format!(
                "ambit.yml and ambit.yaml both exist in {}",
                dir.path().display()
            )
        );
    }

    #[test]
    fn reports_a_project_with_no_config() {
        let dir = tempdir();
        let error = load_project_config(dir.path()).unwrap_err();

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(
            error.message,
            format!("no ambit config in {}", dir.path().display())
        );
    }

    #[test]
    fn names_the_file_it_actually_read_in_errors() {
        let dir = tempdir();

        write_text(
            &dir.path().join("ambit.yaml"),
            "version: 1\nrequires: core\n",
        )
        .unwrap();

        assert!(
            load_project_config(dir.path())
                .unwrap_err()
                .format()
                .contains(
                    "\"requires\" must be a sequence of strings or mappings (ambit.yaml line 2)"
                )
        );
    }
}
