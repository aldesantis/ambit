use super::*;
use crate::errors::ExitCode;
use crate::model::yaml::parse_yaml_mapping;

const FILE: &str = "ambit.yml";

fn parse(text: &str, addressing: Addressing) -> Vec<PatternEntry> {
    parse_entries(&parse_yaml_mapping(text, FILE).unwrap(), addressing).unwrap()
}

fn rejection(text: &str, addressing: Addressing) -> AmbitError {
    let error = parse_entries(&parse_yaml_mapping(text, FILE).unwrap(), addressing)
        .expect_err("expected the entry to be rejected");

    assert_eq!(error.code, ExitCode::Config, "{}", error.format());
    error
}

fn item(kind: ItemKind, catalog: &'static str, name: &'static str) -> PatternItem<'static> {
    PatternItem {
        kind,
        catalog,
        name,
    }
}

fn skill_item() -> PatternItem<'static> {
    item(ItemKind::Skill, "company", "core.a")
}

fn entry(kind: ItemKind, pattern: &str, catalog: Option<&str>) -> PatternEntry {
    PatternEntry {
        kind,
        pattern: pattern.to_owned(),
        catalog: catalog.map(str::to_owned),
    }
}

fn skill(pattern: &str) -> PatternEntry {
    entry(ItemKind::Skill, pattern, None)
}

mod the_glob_matcher {
    use super::*;

    #[test]
    fn treats_a_pattern_with_no_wildcard_as_an_exact_name() {
        assert!(matches_pattern("core", "core"));
        assert!(!matches_pattern("core", "core.a"));
        assert!(!matches_pattern("core", "Core"));
        assert!(!matches_pattern("core", ""));
    }

    #[test]
    fn excludes_the_prefix_itself_from_core_star() {
        assert!(matches_pattern("core.*", "core.a"));
        assert!(matches_pattern("core.*", "core.a.b"));
        assert!(!matches_pattern("core.*", "core"));
        assert!(
            ["core.*", "core"]
                .iter()
                .any(|pattern| matches_pattern(pattern, "core"))
        );
    }

    #[test]
    fn spans_dots_because_a_dot_is_a_character_and_not_a_level_separator() {
        assert!(matches_pattern("core.*", "core.a.b.c"));
        assert!(matches_pattern("*", "core.a.b"));
        assert!(matches_pattern(
            "function.*.develop",
            "function.engineering.develop"
        ));
    }

    #[test]
    fn matches_an_empty_run_so_a_trailing_star_does_not_require_a_suffix() {
        assert!(matches_pattern("core.*", "core."));
        assert!(matches_pattern("core*", "core"));
    }

    #[test]
    fn accepts_a_wildcard_anywhere_any_number_of_times() {
        assert!(matches_pattern("*house-style", "core.house-style"));
        assert!(matches_pattern("*style", "core.house-style"));
        assert!(matches_pattern("*core*", "acme.core.a"));
        assert!(matches_pattern("*.*.*", "a.b.c"));
        assert!(!matches_pattern("*.*.*", "a.b"));
        assert!(matches_pattern("*.*", "a.b"));
        assert!(matches_pattern("**", "anything"));
        assert!(matches_pattern("*guards*hooks*", "guards.hooks"));
        assert!(!matches_pattern("*guards*hooks*", "hooks.guards"));
    }

    #[test]
    fn does_not_let_a_prefix_and_a_suffix_overlap() {
        assert!(!matches_pattern("ab*ba", "aba"));
        assert!(matches_pattern("ab*ba", "abba"));
    }

    #[test]
    fn matches_every_other_metacharacter_literally() {
        assert!(!matches_pattern("core.a", "coreXa"));
        assert!(!matches_pattern("core.*", "coreXa"));
        assert!(matches_pattern("a+b", "a+b"));
        assert!(!matches_pattern("a+b", "aab"));
        assert!(matches_pattern("a(b)", "a(b)"));
        assert!(matches_pattern("a[bc]", "a[bc]"));
        assert!(!matches_pattern("a[bc]", "ab"));
        assert!(matches_pattern("a$", "a$"));
        assert!(matches_pattern("a|b", "a|b"));
        assert!(!matches_pattern("a|b", "a"));
        assert!(matches_pattern("^a", "^a"));
        assert!(!matches_pattern("^a", "a"));
        assert!(matches_pattern("a\\b", "a\\b"));
        assert!(matches_pattern("a?", "a?"));
        assert!(!matches_pattern("a?", "a"));
        assert!(matches_pattern("a{2}", "a{2}"));
        assert!(matches_pattern("a.*+b", "a.x+b"));
        assert!(!matches_pattern("a.*+b", "aXx+b"));
    }

    #[test]
    fn does_not_read_a_bang_as_negation() {
        assert!(!matches_pattern("!core.*", "core.a"));
        assert!(matches_pattern("!core.*", "!core.a"));
    }

    #[test]
    fn anchors_at_both_ends_so_a_pattern_is_never_a_substring_test() {
        assert!(!matches_pattern("core", "xcorex"));
        assert!(!matches_pattern("core.a", "core.a.b"));
    }

    #[test]
    fn lets_a_wildcard_span_a_newline() {
        assert!(matches_pattern("a*b", "a\nb"));
    }
}

mod matching_one_item {
    use super::*;

    #[test]
    fn matches_on_the_namespace_the_entrys_key_named_and_never_on_another() {
        let skill_item = item(ItemKind::Skill, "company", "house-style");

        assert!(matches(
            &entry(ItemKind::Skill, "house-style", None),
            skill_item
        ));
        for kind in [ItemKind::Pack, ItemKind::Mcp, ItemKind::Hook] {
            assert!(!matches(&entry(kind, "house-style", None), skill_item));
        }
    }

    #[test]
    fn keeps_a_pack_and_a_skill_of_one_name_apart() {
        let pack = item(ItemKind::Pack, "company", "core");
        let skill_item = item(ItemKind::Skill, "company", "core");

        assert!(matches(&entry(ItemKind::Pack, "core", None), pack));
        assert!(!matches(&entry(ItemKind::Pack, "core", None), skill_item));
        assert!(matches(&entry(ItemKind::Skill, "core", None), skill_item));
        assert!(!matches(&entry(ItemKind::Skill, "core", None), pack));
    }

    #[test]
    fn restricts_a_qualified_entry_to_its_own_catalog_and_leaves_an_unqualified_one_blind() {
        let qualified = entry(ItemKind::Skill, "*", Some("company"));

        assert!(matches(&qualified, skill_item()));
        assert!(!matches(
            &qualified,
            item(ItemKind::Skill, "personal", "core.a")
        ));

        let unqualified = skill("*");

        assert!(matches(&unqualified, skill_item()));
        assert!(matches(
            &unqualified,
            item(ItemKind::Skill, "personal", "core.a")
        ));
    }

    #[test]
    fn matches_every_namespace_with_the_same_glob_rules_packs_included() {
        for &kind in ITEM_KINDS {
            let wide = entry(kind, "core.*", None);

            assert!(matches(&wide, item(kind, "company", "core.a.b")));
            assert!(!matches(&wide, item(kind, "company", "core")));
        }
    }
}

mod writing_an_entry_back_out {
    use super::*;

    #[test]
    fn recomposes_the_address_it_was_parsed_from() {
        assert_eq!(
            entry_address(&entry(ItemKind::Skill, "core.*", Some("company"))),
            "company/core.*"
        );
        assert_eq!(entry_address(&skill("core.*")), "core.*");
    }

    #[test]
    fn prints_the_namespace_and_the_address_in_ambit_whys_own_shape() {
        assert_eq!(
            format_entry(&entry(ItemKind::Pack, "core", Some("company"))),
            "pack:company/core"
        );
        assert_eq!(format_entry(&skill("core.*")), "skill:core.*");
    }

    #[test]
    fn renders_advice_as_one_block_style_line_which_one_key_fits_on() {
        let yaml = entry_yaml(&entry(ItemKind::Skill, "core.*", Some("company")));

        assert_eq!(yaml, "- skill: \"company/core.*\"");
        assert!(!yaml.contains('\n'));
        assert_eq!(
            parse(&format!("requires:\n  {yaml}\n"), Addressing::Qualified),
            vec![entry(ItemKind::Skill, "core.*", Some("company"))]
        );
    }
}

mod literal_equality_and_deduplication {
    use super::*;

    #[test]
    fn does_not_let_a_wildcard_absorb_a_name_it_matches() {
        let star = skill("core.*");
        let exact = skill("core.a");

        assert!(!same_entry(&star, &exact));
        assert_eq!(unique_entries(&[star, exact]).len(), 2);
    }

    #[test]
    fn separates_the_namespaces_the_catalogs_and_the_patterns() {
        let base = entry(ItemKind::Skill, "x", Some("company"));

        assert!(!same_entry(
            &base,
            &PatternEntry {
                kind: ItemKind::Pack,
                ..base.clone()
            }
        ));
        assert!(!same_entry(
            &base,
            &PatternEntry {
                pattern: "y".to_owned(),
                ..base.clone()
            }
        ));
        assert!(!same_entry(
            &base,
            &PatternEntry {
                catalog: Some("personal".to_owned()),
                ..base.clone()
            }
        ));
        assert!(same_entry(&base, &base.clone()));
        assert!(!same_entry(&base, &skill("x")));
    }

    #[test]
    fn keeps_the_first_of_each_duplicate_and_the_order_the_list_was_written_in() {
        let a = skill("a");
        let b = skill("b");

        assert_eq!(
            unique_entries(&[b.clone(), a.clone(), b.clone(), a.clone()]),
            vec![b, a]
        );
        assert_eq!(unique_entries(&[]), Vec::<PatternEntry>::new());
    }
}

mod parsing_a_requires_list {
    use super::*;

    #[test]
    fn reads_the_designs_own_example_qualified() {
        let entries = parse(
            "requires:
  - pack: \"company/function.engineering\"
  - skill: \"company/core.*\"
  - mcp: \"local/*\"
  - hook: \"company/guards.*\"
",
            Addressing::Qualified,
        );

        assert_eq!(
            entries,
            vec![
                entry(ItemKind::Pack, "function.engineering", Some("company")),
                entry(ItemKind::Skill, "core.*", Some("company")),
                entry(ItemKind::Mcp, "*", Some("local")),
                entry(ItemKind::Hook, "guards.*", Some("company")),
            ]
        );
    }

    #[test]
    fn reads_a_catalogs_own_list_unqualified() {
        assert_eq!(
            parse("requires:\n  - hook: guards\n", Addressing::Unqualified),
            vec![entry(ItemKind::Hook, "guards", None)]
        );
    }

    #[test]
    fn reads_a_pack_requiring_another_pack() {
        assert_eq!(
            parse(
                "requires:\n  - pack: core\n  - skill: code-review\n",
                Addressing::Unqualified
            ),
            vec![entry(ItemKind::Pack, "core", None), skill("code-review")]
        );
    }

    #[test]
    fn treats_an_absent_key_as_no_entries_and_an_empty_list_as_exactly_that() {
        assert_eq!(
            parse("version: 1\n", Addressing::Qualified),
            Vec::<PatternEntry>::new()
        );
        assert_eq!(
            parse("requires: []\n", Addressing::Qualified),
            Vec::<PatternEntry>::new()
        );
    }

    #[test]
    fn keeps_the_order_the_list_was_written_in_duplicates_included() {
        let entries = parse(
            "requires:
  - skill: \"b/*\"
  - skill: \"a/*\"
  - skill: \"b/*\"
",
            Addressing::Qualified,
        );

        assert_eq!(
            entries.iter().map(entry_address).collect::<Vec<_>>(),
            ["b/*", "a/*", "b/*"]
        );
        assert_eq!(
            unique_entries(&entries)
                .iter()
                .map(entry_address)
                .collect::<Vec<_>>(),
            ["b/*", "a/*"]
        );
    }
}

mod refusing_a_malformed_entry {
    use super::*;

    #[test]
    fn refuses_a_bare_pattern_naming_what_it_fails_to_say() {
        let error = rejection("requires:\n  - \"company/core.*\"\n", Addressing::Qualified);

        assert!(error.message.contains("\"company/core.*\""));
        assert!(error.message.contains("line 2"));
        assert!(
            error
                .format()
                .contains("does not say which namespace it selects from")
        );
        assert!(error.format().contains("`pack`, `skill`, `mcp`, `hook`"));
        assert!(error.format().contains("- skill: \"company/core.*\""));
    }

    #[test]
    fn proposes_a_placeholder_alias_rather_than_guessing_one() {
        let error = rejection("requires:\n  - \"core.*\"\n", Addressing::Qualified);

        assert!(error.format().contains("- skill: \"<catalog>/core.*\""));
    }

    #[test]
    fn refuses_a_key_this_grammar_does_not_have_listing_the_four_that_it_does() {
        let error = rejection("requires:\n  - description: hello\n", Addressing::Qualified);

        assert!(
            error
                .message
                .contains("unknown key \"requires[0].description\"")
        );
        assert!(error.message.contains("line 2"));
        assert!(
            error
                .format()
                .contains("accepted keys: hook, mcp, pack, skill")
        );
    }

    #[test]
    fn refuses_an_empty_entry_which_names_no_namespace_at_all() {
        let error = rejection("requires:\n  - {}\n", Addressing::Qualified);

        assert!(error.message.contains("selects from no namespace"));
        assert!(error.format().contains("`pack`, `skill`, `mcp`, `hook`"));
    }

    #[test]
    fn refuses_an_entry_naming_two_namespaces_rather_than_picking_one() {
        let error = rejection(
            "requires:\n  - pack: \"c/a\"\n    skill: \"c/b\"\n",
            Addressing::Qualified,
        );

        assert!(
            error
                .message
                .contains("selects from 2 namespaces: pack, skill")
        );
        assert!(error.message.contains("line 2"));
        assert!(error.format().contains("one entry per namespace"));
    }

    #[test]
    fn refuses_the_two_key_spelling_this_grammar_replaced_as_the_unknown_keys_they_are() {
        let tagged = rejection(
            "requires:\n  - tag: \"c/core\"\n    capabilities: [skills]\n",
            Addressing::Qualified,
        );

        assert!(tagged.message.contains("unknown key \"requires[0].tag\""));
        assert!(
            tagged
                .format()
                .contains("accepted keys: hook, mcp, pack, skill")
        );

        let capped = rejection(
            "requires:\n  - skill: \"c/core\"\n    capabilities: [skills]\n",
            Addressing::Qualified,
        );

        assert!(
            capped
                .message
                .contains("unknown key \"requires[0].capabilities\"")
        );
    }

    #[test]
    fn refuses_a_pattern_that_is_not_a_string_and_an_empty_one() {
        assert!(
            rejection("requires:\n  - skill: 1\n", Addressing::Qualified)
                .message
                .contains("must be a string")
        );
        assert!(
            rejection("requires:\n  - skill: \"\"\n", Addressing::Qualified)
                .message
                .contains("must not be empty")
        );
    }
}

mod the_two_spellings_of_an_address {
    use super::*;

    #[test]
    fn requires_a_qualifier_in_a_project_naming_the_key_and_the_line() {
        let error = rejection("requires:\n  - skill: \"core.*\"\n", Addressing::Qualified);

        assert!(error.message.contains("\"core.*\" names no catalog"));
        assert!(error.message.contains("line 2"));
        assert!(error.format().contains("`<catalog>/core.*`"));
        assert!(error.format().contains("`catalogs:`"));
    }

    #[test]
    fn refuses_a_qualifier_in_a_catalog_saying_why_an_author_cannot_write_one() {
        let error = rejection(
            "requires:\n  - hook: \"company/guards\"\n",
            Addressing::Unqualified,
        );

        assert!(error.message.contains("\"company/guards\" names a catalog"));
        assert!(error.message.contains("line 2"));
        assert!(error.format().contains("belongs to the consumer's config"));
        assert!(error.format().contains("- hook: \"guards\""));
    }

    #[test]
    fn refuses_a_second_separator_in_either_spelling_since_a_name_holds_none() {
        for addressing in [Addressing::Qualified, Addressing::Unqualified] {
            let error = rejection("requires:\n  - skill: \"c/core/a\"\n", addressing);

            assert!(error.message.contains("line 2"));
            assert_eq!(error.code, ExitCode::Config);
        }

        assert!(
            rejection(
                "requires:\n  - skill: \"c/core/a\"\n",
                Addressing::Qualified
            )
            .message
            .contains("2 `/` separators")
        );
    }

    #[test]
    fn refuses_an_empty_half_of_a_qualified_address() {
        assert!(
            rejection("requires:\n  - skill: \"/core.*\"\n", Addressing::Qualified)
                .message
                .contains("names an empty catalog")
        );
        assert!(
            rejection(
                "requires:\n  - skill: \"company/\"\n",
                Addressing::Qualified
            )
            .message
            .contains("names an empty pattern")
        );
    }

    #[test]
    fn takes_catalog_star_as_the_catalog_wide_selector() {
        let entries = parse(
            "requires:\n  - skill: \"company/*\"\n",
            Addressing::Qualified,
        );
        let only = &entries[0];

        assert_eq!(only, &entry(ItemKind::Skill, "*", Some("company")));
        assert!(matches(
            only,
            item(ItemKind::Skill, "company", "anything.at.all")
        ));
        assert!(!matches(only, item(ItemKind::Skill, "personal", "core.a")));
    }

    #[test]
    fn keeps_a_catalog_alias_holding_a_dot_addressable() {
        let entries = parse(
            "requires:\n  - skill: \"my.catalog/core.*\"\n",
            Addressing::Qualified,
        );

        assert_eq!(
            entries,
            vec![entry(ItemKind::Skill, "core.*", Some("my.catalog"))]
        );
    }
}

#[test]
fn lists_the_namespaces_in_report_order_with_packs_first() {
    assert_eq!(
        ITEM_KINDS
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>(),
        ["pack", "skill", "mcp", "hook"]
    );
}
