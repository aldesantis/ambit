//! The shared YAML loader. Every rule here exists because the alternative is silent corruption,
//! so each one is asserted to fail loudly: with the exit code, the offending identifier, and the
//! line.

use serde_json::json;

use super::*;
use crate::errors::ExitCode;
use crate::util::json::JsonValue;
use saphyr::ScalarOwned;

/// Asserts `result` rejected the document as a config error (exit 2), and returns the error.
#[track_caller]
fn rejection<T: std::fmt::Debug>(result: Result<T>) -> AmbitError {
    match result {
        Ok(value) => panic!("expected a rejection, got {value:?}"),
        Err(error) => {
            assert_eq!(
                error.code,
                ExitCode::Config,
                "expected exit 2: {}",
                error.format()
            );
            error
        }
    }
}

#[track_caller]
fn assert_contains(haystack: &str, needle: &str) {
    assert!(
        haystack.contains(needle),
        "expected {haystack:?} to contain {needle:?}"
    );
}

const FILE: &str = "sample.yml";

fn load(text: &str) -> YamlMapping {
    parse_yaml_mapping(text, FILE).unwrap()
}

fn positioned(value: &str, line: usize) -> PositionedString {
    PositionedString {
        value: value.to_owned(),
        line: Some(line),
    }
}

mod loader {
    use super::*;

    #[test]
    fn reads_a_mapping_into_positioned_typed_accessors() {
        let root = load("version: 1\nname: acme\ntags:\n  - core\n  - function.engineering\n");

        assert_eq!(root.file(), FILE);
        assert_eq!(root.keys(), ["version", "name", "tags"]);
        assert_eq!(root.require_integer("version").unwrap(), 1);
        assert_eq!(root.optional_integer("version").unwrap(), Some(1));
        assert_eq!(root.require_string("name").unwrap(), "acme");
        assert_eq!(
            root.optional_string_list("tags").unwrap(),
            Some(vec!["core".to_owned(), "function.engineering".to_owned()])
        );
        assert_eq!(root.line_of("tags"), Some(3));
    }

    #[test]
    fn treats_an_absent_key_as_absent_not_as_a_value() {
        let root = load("version: 1\n");

        assert!(!root.has("tags"));
        assert_eq!(root.optional_string("tags").unwrap(), None);
        assert_eq!(root.optional_integer("tags").unwrap(), None);
        assert_eq!(root.optional_string_list("tags").unwrap(), None);
        assert!(root.optional_mapping("tags").unwrap().is_none());
        assert!(root.optional_mapping_list("tags").unwrap().is_none());
        assert!(root.optional_entry_list("tags").unwrap().is_none());
    }

    #[test]
    fn distinguishes_an_empty_sequence_from_an_absent_one() {
        assert_eq!(
            load("tags: []\n").optional_string_list("tags").unwrap(),
            Some(vec![])
        );
    }

    #[test]
    fn pairs_each_sequence_item_with_its_own_line_block_style_and_flow_style_alike() {
        // A rule enforced after parsing has no node left to point at, so the position has to come
        // out of the document with the value.
        assert_eq!(
            load("tags:\n  - core\n  - function.engineering\n")
                .optional_positioned_string_list("tags")
                .unwrap(),
            Some(vec![
                positioned("core", 2),
                positioned("function.engineering", 3)
            ])
        );
        assert_eq!(
            load("version: 1\ntags: [core]\n")
                .optional_positioned_string_list("tags")
                .unwrap(),
            Some(vec![positioned("core", 2)])
        );
    }

    #[test]
    fn rejects_a_duplicate_key_naming_both_lines() {
        let error = rejection(parse_yaml_mapping(
            "version: 1\ntags:\n  - core\ntags:\n  - other\n",
            FILE,
        ));

        assert_contains(
            &error.format(),
            &format!("duplicate key \"tags\" ({FILE} line 4)"),
        );
        assert_contains(&error.format(), "first defined on line 2");
    }

    #[test]
    fn rejects_tabs_used_for_indentation() {
        let error = rejection(parse_yaml_mapping(
            "version: 1\nharnesses:\n\t- claude\n",
            FILE,
        ));

        assert_contains(
            &error.format(),
            &format!("YAML does not permit tabs for indentation ({FILE} line 3)"),
        );
    }

    #[test]
    fn rejects_custom_tags_shorthand_and_local_alike() {
        assert_contains(
            &rejection(parse_yaml_mapping(
                "version: 1\nthing: !!python/object:x {}\n",
                FILE,
            ))
            .format(),
            "custom YAML tag `!!python/object:x` is not permitted (sample.yml line 2)",
        );
        assert_contains(
            &rejection(parse_yaml_mapping("thing: !mine value\n", FILE)).format(),
            "custom YAML tag `!mine` is not permitted (sample.yml line 1)",
        );
    }

    #[test]
    fn accepts_the_core_schemas_own_tags() {
        assert_eq!(
            load("ref: !!str 1234567\n").require_string("ref").unwrap(),
            "1234567"
        );
    }

    #[test]
    fn rejects_an_empty_document_rather_than_reading_it_as_an_empty_mapping() {
        assert_contains(
            &rejection(parse_yaml_mapping("", FILE)).format(),
            &format!("{FILE} is empty"),
        );
        assert_contains(
            &rejection(parse_yaml_mapping("# just a comment\n", FILE)).format(),
            &format!("{FILE} is empty"),
        );
    }

    #[test]
    fn rejects_a_document_whose_root_is_not_a_mapping() {
        assert_contains(
            &rejection(parse_yaml_mapping("- core\n- other\n", FILE)).format(),
            &format!("root is not a mapping ({FILE} line 1)"),
        );
        assert_contains(
            &rejection(parse_yaml_mapping("hello\n", FILE)).format(),
            "found a string at the document root",
        );
    }

    #[test]
    fn rejects_a_non_string_mapping_key() {
        let error = rejection(parse_yaml_mapping("version: 1\n1: two\n", FILE));

        assert_contains(
            &error.format(),
            &format!("mapping keys must be strings ({FILE} line 2)"),
        );
    }

    #[test]
    fn reports_a_syntax_error_with_its_line() {
        let error = rejection(parse_yaml_mapping("version: 1\n  bad: indentation\n", FILE));
        let pattern = regex::Regex::new(&format!(r"invalid YAML \({FILE} line \d+\)")).unwrap();

        assert!(pattern.is_match(&error.format()), "{}", error.format());
    }

    /// Where saphyr notices a problem after the line that holds it, the line is moved back to it.
    #[test]
    fn reports_a_syntax_error_on_the_line_that_holds_it() {
        for (text, line) in [
            ("version: 1\ncatalogs: [\n", 3),
            ("version: 1\ncatalogs: [", 2),
            ("version: 1\ntampered\n", 2),
            ("version: 1\ntampered", 2),
            ("version: 1\n\n\n\nfoo\n\n\n", 5),
            ("version: 1\nfoo: bar\nbaz\nqux: 1\n", 3),
            ("version: 1\nname: \"unterminated\n", 3),
            ("version: 1\nharnesses:\n  - a\n tampered\n", 4),
        ] {
            assert_contains(
                &rejection(parse_yaml_mapping(text, FILE)).format(),
                &format!("invalid YAML ({FILE} line {line})"),
            );
        }
    }

    #[test]
    fn reports_a_frontmatter_syntax_error_on_the_line_that_holds_it() {
        assert_contains(
            &rejection(parse_frontmatter_mapping("---\nname: [\n---\n", FILE)).format(),
            &format!("invalid YAML ({FILE} line 2)"),
        );
    }

    #[test]
    fn rejects_more_than_one_document() {
        assert_contains(
            &rejection(parse_yaml_mapping("a: 1\n---\nb: 2\n", FILE)).format(),
            &format!("invalid YAML ({FILE} line 2)"),
        );
    }

    #[test]
    fn reads_an_alias_as_no_supported_value() {
        let root = load("a: &x one\nb: *x\n");

        assert_eq!(root.require_string("a").unwrap(), "one");
        assert_contains(
            &rejection(root.require_string("b")).format(),
            "found an unsupported value",
        );
    }
}

mod string_values {
    use super::*;

    #[test]
    fn rejects_a_number_where_a_string_is_required_quoting_the_original_text() {
        let error = rejection(load("ref: 1234567\n").require_string("ref"));

        assert_contains(
            &error.format(),
            &format!("\"ref\" must be a string ({FILE} line 1)"),
        );
        assert_contains(&error.format(), "YAML parsed `1234567` as a number");
        assert_contains(&error.format(), "quote it: `ref: \"1234567\"`");
    }

    #[test]
    fn suggests_the_fix_in_the_form_it_was_written_not_the_parsed_value() {
        // `1e5` parses to 100000; suggesting `ref: "100000"` would point at a different commit.
        assert_contains(
            &rejection(load("ref: 1e5\n").require_string("ref")).format(),
            "quote it: `ref: \"1e5\"`",
        );
    }

    #[test]
    fn rejects_a_boolean_where_a_string_is_required() {
        assert_contains(
            &rejection(load("name: true\n").require_string("name")).format(),
            "YAML parsed `true` as a boolean",
        );
    }

    #[test]
    fn rejects_an_empty_string() {
        assert_contains(
            &rejection(load("name: \"\"\n").require_string("name")).format(),
            &format!("\"name\" must not be empty ({FILE} line 1)"),
        );
    }

    #[test]
    fn names_sequence_items_by_index_and_suggests_a_pastable_fix() {
        let error = rejection(load("tags:\n  - 1234\n").optional_string_list("tags"));

        assert_contains(
            &error.format(),
            &format!("\"tags[0]\" must be a string ({FILE} line 2)"),
        );
        assert_contains(&error.format(), "quote it: `- \"1234\"`");
    }
}

mod required_and_null_values {
    use super::*;

    #[test]
    fn reports_a_missing_required_key() {
        let error = rejection(load("tags: [core]\n").require_string("name"));

        assert_contains(
            &error.format(),
            &format!("missing required key \"name\" ({FILE} line 1)"),
        );
        assert_contains(&error.format(), "add `name:` with a value");
    }

    #[test]
    fn rejects_an_explicit_null_where_a_value_is_required() {
        let error = rejection(load("version: null\n").require_integer("version"));

        assert_contains(
            &error.format(),
            &format!("\"version\" must not be null ({FILE} line 1)"),
        );
        assert_contains(&error.format(), "give it a value");
    }

    #[test]
    fn rejects_an_explicit_null_on_an_optional_key_pointing_at_omission_instead() {
        let error = rejection(load("tags: ~\n").optional_string_list("tags"));

        assert_contains(
            &error.format(),
            &format!("\"tags\" must not be null ({FILE} line 1)"),
        );
        assert_contains(&error.format(), "remove the key to take its default");
    }

    #[test]
    fn rejects_a_key_written_with_no_value_at_all() {
        assert_contains(
            &rejection(load("name:\n").require_string("name")).format(),
            &format!("\"name\" must not be null ({FILE} line 1)"),
        );
    }
}

mod unknown_keys {
    use super::*;

    #[test]
    fn rejects_them_listing_what_is_accepted() {
        let error =
            rejection(load("version: 1\ntag: core\n").reject_unknown_keys(&["version", "tags"]));

        assert_contains(
            &error.format(),
            &format!("unknown key \"tag\" ({FILE} line 2)"),
        );
        assert_contains(&error.format(), "accepted keys: tags, version");
    }

    #[test]
    fn labels_a_nested_unknown_key_by_its_path() {
        let error = rejection(
            load("transport:\n  stdio:\n    cmd: npx\n")
                .require_mapping("transport")
                .unwrap()
                .require_mapping("stdio")
                .unwrap()
                .reject_unknown_keys(&["args", "command"]),
        );

        assert_contains(&error.format(), "unknown key \"transport.stdio.cmd\"");
    }
}

mod structured_values {
    use super::*;

    #[test]
    fn reads_nested_mappings_keeping_the_path_for_errors() {
        let root = load("transport:\n  http:\n    url: https://acme.invalid/mcp\n");
        let http = root
            .require_mapping("transport")
            .unwrap()
            .require_mapping("http")
            .unwrap();

        assert_eq!(
            http.require_string("url").unwrap(),
            "https://acme.invalid/mcp"
        );
        assert_contains(
            &rejection(http.require_string("method")).format(),
            "\"transport.http.method\"",
        );
    }

    #[test]
    fn reads_a_free_form_string_map() {
        let headers = load("headers:\n  Authorization: \"Bearer ${TOKEN}\"\n  X-Trace: \"1\"\n")
            .require_mapping("headers")
            .unwrap()
            .string_entries()
            .unwrap();

        assert_eq!(
            headers,
            IndexMap::from([
                ("Authorization".to_owned(), "Bearer ${TOKEN}".to_owned()),
                ("X-Trace".to_owned(), "1".to_owned()),
            ])
        );
    }

    #[test]
    fn reads_a_list_of_mappings_indexing_the_path() {
        let catalogs = load("catalogs:\n  - name: one\n  - name: two\n")
            .optional_mapping_list("catalogs")
            .unwrap()
            .unwrap();
        let names: Vec<String> = catalogs
            .iter()
            .map(|entry| entry.require_string("name").unwrap())
            .collect();

        assert_eq!(names, ["one", "two"]);
        assert_contains(
            &rejection(catalogs[1].require_string("source")).format(),
            "\"catalogs[1].source\"",
        );
    }

    #[test]
    fn rejects_a_non_mapping_in_a_list_of_mappings() {
        let error =
            rejection(load("catalogs:\n  - acme/skills\n").optional_mapping_list("catalogs"));

        assert_contains(
            &error.format(),
            &format!("\"catalogs[0]\" must be a mapping ({FILE} line 2)"),
        );
    }

    #[test]
    fn reads_a_list_of_strings_or_mappings_positioning_the_bare_names() {
        let entries = load("requires:\n  - house-style\n  - name: two\n")
            .optional_entry_list("requires")
            .unwrap()
            .unwrap();

        assert!(
            matches!(&entries[0], YamlEntry::String(entry) if *entry == positioned("house-style", 2))
        );
        assert!(matches!(&entries[1], YamlEntry::Mapping(_)));
    }

    #[test]
    fn rejects_an_entry_that_is_neither_a_string_nor_a_mapping() {
        let error = rejection(load("requires:\n  - [nested]\n").optional_entry_list("requires"));

        assert_contains(
            &error.format(),
            &format!("\"requires[0]\" must be a string or a mapping ({FILE} line 2)"),
        );
    }

    #[test]
    fn rejects_a_scalar_where_a_sequence_belongs() {
        let error = rejection(load("tags: core\n").optional_string_list("tags"));

        assert_contains(
            &error.format(),
            &format!("\"tags\" must be a sequence of strings ({FILE} line 1)"),
        );
    }

    #[test]
    fn rejects_a_scalar_where_a_mapping_belongs() {
        assert_contains(
            &rejection(load("transport: stdio\n").require_mapping("transport")).format(),
            &format!("\"transport\" must be a mapping ({FILE} line 1)"),
        );
    }
}

mod integers {
    use super::*;

    #[test]
    fn rejects_a_string_that_looks_like_one() {
        assert_contains(
            &rejection(load("version: \"1\"\n").require_integer("version")).format(),
            &format!("\"version\" must be an integer ({FILE} line 1)"),
        );
    }

    #[test]
    fn rejects_a_non_integer_number() {
        assert_contains(
            &rejection(load("version: 1.5\n").require_integer("version")).format(),
            "found a number",
        );
    }
}

mod frontmatter {
    use super::*;

    const DOC: &str = "SKILL.md";

    fn frontmatter(text: &str) -> Result<YamlMapping> {
        parse_frontmatter_mapping(text, DOC)
    }

    #[test]
    fn parses_the_block_under_the_same_rules_as_a_file() {
        let root =
            frontmatter("---\nname: close-crm\ntags: [function.sales]\n---\n\n# Close\n").unwrap();

        assert_eq!(root.keys(), ["name", "tags"]);
        assert_eq!(root.require_string("name").unwrap(), "close-crm");
        assert_eq!(
            root.optional_string_list("tags").unwrap(),
            Some(vec!["function.sales".to_owned()])
        );
    }

    #[test]
    fn parses_an_indented_block() {
        let root = frontmatter("---\n  name: a\n  description: b\n---\n").unwrap();

        assert_eq!(root.keys(), ["name", "description"]);
    }

    #[test]
    fn reports_lines_of_the_document_not_of_the_block() {
        // The reader is told a line number to go to, so it has to be the document's own.
        let text = "---\nname: alpha\nref: 1e5\n---\nbody\n";

        assert_contains(
            &rejection(frontmatter(text).unwrap().require_string("ref")).format(),
            &format!("({DOC} line 3)"),
        );
    }

    #[test]
    fn applies_every_rule_to_the_block() {
        let duplicate = "---\nname: a\nname: b\n---\n";

        assert_eq!(
            rejection(frontmatter(duplicate)).message,
            format!("duplicate key \"name\" ({DOC} line 3)")
        );

        let tabbed = "---\ntags:\n\t- core\n---\n";

        assert_contains(
            &rejection(frontmatter(tabbed)).message,
            "does not permit tabs",
        );
    }

    #[test]
    fn rejects_a_document_with_no_block() {
        assert_eq!(
            rejection(frontmatter("# Close\n")).message,
            format!("{DOC} has no frontmatter block")
        );
    }

    #[test]
    fn rejects_a_block_holding_nothing() {
        for text in ["---\n---\n", "---\n\n---\n", "---\n# only a comment\n---\n"] {
            assert_eq!(
                rejection(frontmatter(text)).message,
                format!("{DOC} has an empty frontmatter block")
            );
        }
    }

    #[test]
    fn rejects_a_non_mapping_block() {
        assert_contains(
            &rejection(frontmatter("---\n- core\n---\n")).message,
            "root is not a mapping",
        );
    }

    #[test]
    fn treats_a_language_tag_as_no_block() {
        for text in [
            "---json\n{\"name\": \"a\"}\n---\n",
            "---yaml\nname: a\n---\n",
        ] {
            assert_eq!(
                rejection(frontmatter(text)).message,
                format!("{DOC} has no frontmatter block")
            );
        }
    }

    #[test]
    fn rejects_an_unclosed_block() {
        assert_eq!(
            rejection(frontmatter("---\nname: a\n")).message,
            format!("{DOC} has an unclosed frontmatter block")
        );
    }

    #[test]
    fn reads_a_file_and_names_it_as_asked_in_errors() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("SKILL.md");

        std::fs::write(&target, "---\nname: alpha\n---\n").unwrap();

        assert_eq!(read_frontmatter_mapping(&target, DOC).unwrap().file(), DOC);
    }
}

mod read_yaml_mapping {
    use super::*;

    #[test]
    fn reads_a_file_and_names_it_as_asked_in_errors() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("entity.yml");

        std::fs::write(&target, "tags:\n  core: {}\n").unwrap();

        let root = super::super::read_yaml_mapping(&target, "entity.yml").unwrap();

        assert_eq!(root.file(), "entity.yml");
        assert_eq!(root.keys(), ["tags"]);
    }

    #[test]
    fn reports_an_unreadable_file_as_a_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("absent.yml");
        let error = rejection(super::super::read_yaml_mapping(&missing, "absent.yml"));

        assert_eq!(error.message, "cannot read absent.yml");
    }
}

/// The emit half of the rules. They are about *bytes*, not about meaning, because the two
/// artifacts that use them (`ambit.lock` and the `init` scaffold) are diffed and compared as text.
/// So each test asserts the exact output rather than that it re-parses.
mod emitter {
    use super::*;

    #[test]
    fn sorts_keys_at_every_depth_whatever_order_they_were_built_in() {
        assert_eq!(
            emit_yaml(&json!({
                "version": 1,
                "catalogs": { "personal": { "source": "b" }, "company": { "source": "a" } }
            })),
            "catalogs:\n  company:\n    source: a\n  personal:\n    source: b\nversion: 1\n"
        );
    }

    #[test]
    fn quotes_a_string_a_core_schema_parser_would_otherwise_read_as_a_number() {
        // The trap the rules name: an all-digit commit SHA, and a ref like `1e5`. Both must come
        // back as the strings they went in as, or the lock pins a different commit than the one
        // installed.
        let text = emit_yaml(&json!({ "commit": "1234567", "ref": "1e5", "count": 12 }));

        assert_eq!(text, "commit: \"1234567\"\ncount: 12\nref: \"1e5\"\n");

        let parsed = load(&text);

        assert_eq!(parsed.require_string("commit").unwrap(), "1234567");
        assert_eq!(parsed.require_string("ref").unwrap(), "1e5");
        assert_eq!(parsed.require_integer("count").unwrap(), 12);
    }

    #[test]
    fn writes_no_anchor_for_two_entries_that_happen_to_hold_equal_values() {
        let shared = json!({ "catalog": "company" });

        assert_eq!(
            emit_yaml(&json!({ "a": shared, "b": shared })),
            "a:\n  catalog: company\nb:\n  catalog: company\n"
        );
    }

    #[test]
    fn keeps_a_long_value_on_its_own_line_rather_than_wrapping_it() {
        let url = format!("https://example.invalid/{}", "a".repeat(200));

        assert_eq!(emit_yaml(&json!({ "url": url })), format!("url: {url}\n"));
    }

    #[test]
    fn escapes_an_awkward_value_inline_rather_than_reaching_for_a_block_scalar() {
        assert_eq!(
            emit_yaml(&json!({ "description": "two\nlines" })),
            "description: \"two\\nlines\"\n"
        );
    }

    #[test]
    fn emits_an_empty_mapping_rather_than_dropping_the_key() {
        assert_eq!(
            emit_yaml(&json!({ "mcps": {}, "version": 1 })),
            "mcps: {}\nversion: 1\n"
        );
    }

    #[test]
    fn is_byte_stable_across_calls() {
        let document = json!({ "skills": { "beta": { "path": "b" }, "alpha": { "path": "a" } }, "version": 1 });

        assert_eq!(emit_yaml(&document), emit_yaml(&document));
    }

    /// Converts a parsed node back to the JSON value it holds.
    fn to_json(document: &load::Document, node: &load::Node) -> JsonValue {
        if document.is_map(node) {
            return JsonValue::Object(
                document
                    .pairs(node)
                    .into_iter()
                    .map(|(key, value)| {
                        let key = document.string(key).expect("a string key").to_owned();

                        (key, to_json(document, value))
                    })
                    .collect(),
            );
        }

        if let Some(items) = document.items(node) {
            return items.iter().map(|item| to_json(document, item)).collect();
        }

        match document.scalar(node).expect("a scalar") {
            ScalarOwned::Null => JsonValue::Null,
            ScalarOwned::Boolean(b) => json!(b),
            ScalarOwned::Integer(n) => json!(n),
            ScalarOwned::FloatingPoint(n) => json!(n.0),
            ScalarOwned::String(text) => json!(text),
        }
    }

    /// Every value in the corpus must read back as itself, and emit as the recorded bytes so a
    /// change in layout shows up as a diff of the corpus. `UPDATE_GOLDEN=1 cargo test` rewrites
    /// the recorded bytes.
    #[test]
    fn round_trips_and_matches_the_recorded_corpus() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/yaml_emit.json");
        let mut corpus: Vec<JsonValue> =
            serde_json::from_str(include_str!("../../../tests/fixtures/yaml_emit.json")).unwrap();
        let mut failures = Vec::new();

        for case in &mut corpus {
            let actual = emit_yaml(&case["input"]);
            let mut document = load::parse(&actual, FILE, 0).unwrap();
            let root = document.root.take().unwrap();

            if to_json(&document, &root) != case["input"] {
                failures.push(format!(
                    "{actual:?} does not read back as {}",
                    case["input"]
                ));
            }

            if case["output"].as_str() != Some(&actual) {
                failures.push(format!(
                    "input {}: expected {:?}, got {actual:?}",
                    case["input"], case["output"]
                ));
                case["output"] = json!(actual);
            }
        }

        #[allow(clippy::disallowed_methods)]
        if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
            let mut text = serde_json::to_string_pretty(&corpus).unwrap();

            text.push('\n');
            std::fs::write(path, text).unwrap();

            return;
        }

        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
