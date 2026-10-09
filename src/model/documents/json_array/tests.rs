use indexmap::IndexSet;
use serde_json::json;

use super::*;
use crate::model::documents::{DocumentFormat, DocumentShape, driver_for};
use crate::util::json::{parse, stringify_pretty};

const SECTION: &str = "hooks";
const FILE: &str = ".claude/settings.json";

fn driver() -> ArraySectionDriver {
    array_section_driver(None)
}

fn format_value() -> JsonValue {
    json!({
        "matcher": "Edit",
        "hooks": [{ "type": "command", "command": "npx prettier --write" }],
    })
}

fn format_entry() -> ConfigEntry {
    ConfigEntry {
        key: array_entry_key("PostToolUse", &format_value()),
        value: format_value(),
    }
}

fn greet_value() -> JsonValue {
    json!({ "hooks": [{ "type": "command", "command": "./greet.sh" }] })
}

fn greet_entry() -> ConfigEntry {
    ConfigEntry {
        key: array_entry_key("Stop", &greet_value()),
        value: greet_value(),
    }
}

const HANDWRITTEN: &str = r#"{
  "model": "opus",
  "permissions": {
    "allow": [
      "Bash(git status)"
    ]
  },
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Write",
        "hooks": [
          {
            "type": "command",
            "command": "./mine.sh"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "./welcome.sh"
          }
        ]
      }
    ]
  }
}
"#;

fn merge(text: Option<&str>, entries: &[ConfigEntry]) -> String {
    driver()
        .merge_section(text, SECTION, entries, FILE)
        .expect("merge succeeds")
}

fn remove(driver: &ArraySectionDriver, text: &str, keys: &[String]) -> String {
    driver
        .remove_keys(Some(text), SECTION, keys, FILE)
        .expect("removal succeeds")
        .expect("something was removed")
}

fn parsed(text: &str) -> JsonObject {
    parse(text)
        .expect("valid JSON")
        .as_object()
        .expect("an object")
        .clone()
}

fn hooks_of(text: &str) -> JsonObject {
    parsed(text)[SECTION]
        .as_object()
        .expect("an object")
        .clone()
}

fn keys(object: &JsonObject) -> Vec<&str> {
    object.keys().map(String::as_str).collect()
}

fn set(items: &[String]) -> IndexSet<String> {
    items.iter().cloned().collect()
}

mod the_managed_key {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn is_the_event_and_the_first_twelve_hex_of_the_entrys_digest() {
        let key = format_entry().key;
        let (event, digest) = key.split_once('@').expect("an @");

        assert_eq!(event, "PostToolUse");
        assert_eq!(digest.len(), 12);
        assert!(
            digest
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }

    #[test]
    fn is_a_function_of_the_entry_alone_so_two_machines_agree() {
        assert_eq!(
            entry_digest(&format_value()),
            entry_digest(&format_value().clone())
        );
        assert_ne!(entry_digest(&format_value()), entry_digest(&greet_value()));
    }

    #[test]
    fn digests_the_entry_in_the_order_the_renderer_built_it() {
        let reordered = json!({
            "hooks": format_value()["hooks"],
            "matcher": format_value()["matcher"],
        });

        assert_ne!(entry_digest(&reordered), entry_digest(&format_value()));
    }

    #[test]
    fn matches_the_recorded_digests() {
        let corpus: JsonValue = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/json_stringify.json"
        ))
        .expect("valid fixture");
        let cases = corpus.as_array().expect("a cases array");

        assert_ne!(cases.len(), 0);

        for case in cases {
            let source = case["source"].as_str().expect("a source");
            let value = parse(source).expect("the source parses");

            assert_eq!(
                entry_digest(&value),
                case["digest"].as_str().expect("a digest"),
                "{source}"
            );
        }
    }
}

mod merging_a_hook_into_hand_written_settings {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn appends_to_the_event_array_and_leaves_every_other_byte_identical() {
        assert_eq!(
            merge(Some(HANDWRITTEN), &[format_entry()]),
            r#"{
  "model": "opus",
  "permissions": {
    "allow": [
      "Bash(git status)"
    ]
  },
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Write",
        "hooks": [
          {
            "type": "command",
            "command": "./mine.sh"
          }
        ]
      },
      {
        "matcher": "Edit",
        "hooks": [
          {
            "type": "command",
            "command": "npx prettier --write"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "./welcome.sh"
          }
        ]
      }
    ]
  }
}
"#
        );
    }

    #[test]
    fn keeps_the_foreign_entry_first_and_keeps_it_whole() {
        let hooks = hooks_of(&merge(Some(HANDWRITTEN), &[format_entry()]));

        assert_eq!(
            hooks["PostToolUse"],
            json!([
                { "matcher": "Write", "hooks": [{ "type": "command", "command": "./mine.sh" }] },
                format_value(),
            ])
        );
        assert_eq!(
            hooks["SessionStart"],
            json!([{ "hooks": [{ "type": "command", "command": "./welcome.sh" }] }])
        );
    }

    #[test]
    fn creates_an_event_array_the_document_has_none_of_and_keeps_the_ones_it_has() {
        let hooks = hooks_of(&merge(Some(HANDWRITTEN), &[format_entry(), greet_entry()]));

        assert_eq!(keys(&hooks), ["PostToolUse", "SessionStart", "Stop"]);
        assert_eq!(hooks["Stop"], json!([greet_value()]));
    }

    #[test]
    fn writes_a_whole_document_when_the_file_does_not_exist_yet() {
        assert_eq!(
            merge(None, &[format_entry()]),
            format!(
                "{}\n",
                stringify_pretty(&json!({ "hooks": { "PostToolUse": [format_value()] } }))
            )
        );
    }

    #[test]
    fn creates_the_section_in_a_document_that_has_none() {
        let merged = merge(Some("{\n  \"model\": \"opus\"\n}\n"), &[format_entry()]);

        assert_eq!(
            JsonValue::Object(parsed(&merged)),
            json!({ "model": "opus", "hooks": { "PostToolUse": [format_value()] } })
        );
    }

    #[test]
    fn is_idempotent_a_second_merge_of_the_same_entries_is_a_no_op() {
        let once = merge(Some(HANDWRITTEN), &[format_entry(), greet_entry()]);

        assert_eq!(merge(Some(&once), &[format_entry(), greet_entry()]), once);
    }

    #[test]
    fn does_not_append_a_second_copy_of_an_entry_a_person_wrote_by_hand() {
        let by_hand = merge(None, &[format_entry()]);

        assert_eq!(merge(Some(&by_hand), &[format_entry()]), by_hand);
    }
}

mod removing_hooks {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn drops_only_the_matching_digests_and_leaves_the_foreign_entry_in_place() {
        let once = merge(Some(HANDWRITTEN), &[format_entry(), greet_entry()]);

        let removed = remove(&driver(), &once, &[format_entry().key, greet_entry().key]);

        assert_eq!(
            hooks_of(&removed)["PostToolUse"],
            json!([{ "matcher": "Write", "hooks": [{ "type": "command", "command": "./mine.sh" }] }])
        );
        assert_eq!(
            hooks_of(&removed)["SessionStart"],
            json!([{ "hooks": [{ "type": "command", "command": "./welcome.sh" }] }])
        );
        assert_eq!(parsed(&removed)["model"], json!("opus"));
        assert_eq!(
            parsed(&removed)["permissions"],
            json!({ "allow": ["Bash(git status)"] })
        );
    }

    #[test]
    fn leaves_an_emptied_event_array_behind_rather_than_deleting_it() {
        let once = merge(Some(HANDWRITTEN), &[greet_entry()]);

        let removed = remove(&driver(), &once, &[greet_entry().key]);

        assert_eq!(hooks_of(&removed)["Stop"], json!([]));
    }

    #[test]
    fn reports_nothing_to_do_rather_than_rewriting_a_file_it_would_not_change() {
        let driver = driver();
        let format_key = [format_entry().key];

        assert_eq!(
            driver.remove_keys(Some(HANDWRITTEN), SECTION, &format_key, FILE),
            Ok(None)
        );
        assert_eq!(
            driver.remove_keys(Some(HANDWRITTEN), SECTION, &[greet_entry().key], FILE),
            Ok(None)
        );
        assert_eq!(
            driver.remove_keys(Some("{\"model\": \"opus\"}\n"), SECTION, &format_key, FILE),
            Ok(None)
        );
        assert_eq!(
            driver.remove_keys(None, SECTION, &format_key, FILE),
            Ok(None)
        );
    }
}

mod reading_the_section {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn derives_every_key_from_the_file_alone_foreign_entries_included() {
        assert_eq!(
            driver().section_keys(Some(HANDWRITTEN), SECTION, FILE),
            Ok(set(&[
                array_entry_key(
                    "PostToolUse",
                    &json!({
                        "matcher": "Write",
                        "hooks": [{ "type": "command", "command": "./mine.sh" }],
                    })
                ),
                array_entry_key(
                    "SessionStart",
                    &json!({ "hooks": [{ "type": "command", "command": "./welcome.sh" }] })
                ),
            ]))
        );
    }

    #[test]
    fn names_ambits_entry_once_it_is_merged_in() {
        let keys = driver()
            .section_keys(
                Some(&merge(Some(HANDWRITTEN), &[format_entry()])),
                SECTION,
                FILE,
            )
            .expect("readable");

        assert!(keys.contains(&format_entry().key));
    }

    #[test]
    fn reads_an_absent_file_an_absent_section_and_an_unusable_one_as_holding_none() {
        let driver = driver();

        for text in [
            None,
            Some("{\"model\": \"opus\"}"),
            Some("{\"hooks\": []}"),
            Some("{\"hooks\": {\"Stop\": \"nope\"}}"),
        ] {
            assert_eq!(
                driver.section_keys(text, SECTION, FILE),
                Ok(IndexSet::new())
            );
        }
    }
}

mod whether_an_entry_is_already_what_install_would_write {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn says_yes_for_the_entry_install_wrote() {
        let merged = merge(Some(HANDWRITTEN), &[format_entry()]);

        assert_eq!(
            driver().entry_matches(Some(&merged), SECTION, &format_entry(), FILE),
            Ok(true)
        );
    }

    #[test]
    fn says_no_for_an_absent_file_an_absent_entry_and_one_edited_by_hand() {
        let edited = merge(Some(HANDWRITTEN), &[format_entry()]).replacen(
            "npx prettier --write",
            "npx prettier --check",
            1,
        );
        let driver = driver();

        for text in [None, Some(HANDWRITTEN), Some(edited.as_str())] {
            assert_eq!(
                driver.entry_matches(text, SECTION, &format_entry(), FILE),
                Ok(false)
            );
        }
    }
}

mod root_defaults {
    use super::*;
    use pretty_assertions::assert_eq;

    fn versioned() -> ArraySectionDriver {
        let defaults = json!({ "version": 1 });

        array_section_driver(defaults.as_object())
    }

    #[test]
    fn adds_the_key_when_ambit_creates_the_file() {
        assert_eq!(
            versioned().merge_section(None, SECTION, &[format_entry()], FILE),
            Ok(format!(
                "{}\n",
                stringify_pretty(
                    &json!({ "version": 1, "hooks": { "PostToolUse": [format_value()] } })
                )
            ))
        );
    }

    #[test]
    fn never_overwrites_a_version_someone_else_wrote() {
        let theirs = "{\n  \"version\": 2,\n  \"hooks\": {}\n}\n";

        let merged = versioned()
            .merge_section(Some(theirs), SECTION, &[format_entry()], FILE)
            .expect("merge succeeds");

        assert_eq!(parsed(&merged)["version"], json!(2));
        assert_eq!(keys(&parsed(&merged)), ["version", "hooks"]);
    }

    #[test]
    fn adds_nothing_on_a_removal() {
        let once = merge(Some(HANDWRITTEN), &[greet_entry()]);

        let removed = remove(&versioned(), &once, &[greet_entry().key]);

        assert!(!parsed(&removed).contains_key("version"));
        assert_eq!(keys(&parsed(&removed)), ["model", "permissions", "hooks"]);
    }
}

mod selecting_the_driver {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn takes_the_shape_since_the_format_cannot_tell_the_two_json_files_apart() {
        let array = driver_for(DocumentFormat::Json, DocumentShape::Array, None).expect("a driver");

        assert_eq!(
            array.merge_section(None, SECTION, &[format_entry()], FILE),
            Ok(merge(None, &[format_entry()]))
        );

        let map = driver_for(DocumentFormat::Json, DocumentShape::Map, None).expect("a driver");
        let entry = ConfigEntry {
            key: "PostToolUse".to_owned(),
            value: format_value(),
        };

        assert_eq!(
            hooks_of(
                &map.merge_section(None, SECTION, &[entry], FILE)
                    .expect("merge")
            )["PostToolUse"],
            format_value()
        );
    }

    #[test]
    fn hands_the_root_defaults_it_is_given_to_the_driver_it_builds() {
        let defaults = json!({ "version": 1 });
        let merged = driver_for(
            DocumentFormat::Json,
            DocumentShape::Array,
            defaults.as_object(),
        )
        .expect("a driver")
        .merge_section(None, SECTION, &[format_entry()], FILE)
        .expect("merge succeeds");

        assert_eq!(parsed(&merged)["version"], json!(1));
        assert!(!parsed(&merge(None, &[format_entry()])).contains_key("version"));
    }

    #[test]
    fn refuses_a_format_with_no_array_section_driver() {
        for format in [DocumentFormat::Toml, DocumentFormat::Jsonc] {
            let Err(error) = driver_for(format, DocumentShape::Array, None) else {
                panic!("expected a refusal for {format}");
            };

            assert_eq!(error.code, ExitCode::Internal);
            assert_eq!(
                error.message,
                format!("no {format} driver for an array-shaped section")
            );
        }
    }
}

mod what_it_refuses {
    use super::*;
    use pretty_assertions::assert_eq;

    fn refusal(text: &str, entries: &[ConfigEntry]) -> AmbitError {
        driver()
            .merge_section(Some(text), SECTION, entries, FILE)
            .expect_err("expected a refusal")
    }

    #[test]
    fn refuses_a_document_it_cannot_parse_in_the_map_drivers_words() {
        let error = refusal("{ not json\n", &[format_entry()]);

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(error.message, format!("{FILE} is not valid JSON"));
    }

    #[test]
    fn refuses_a_section_holding_something_other_than_an_object() {
        let error = refusal("{\"hooks\": []}\n", &[format_entry()]);

        assert_eq!(
            error.message,
            format!("\"hooks\" in {FILE} is not a JSON object")
        );
        assert_eq!(
            error.detail,
            [
                "ambit appends its entries to the arrays inside `hooks`",
                "make `hooks` an object, or move its current value aside",
            ]
        );
    }

    #[test]
    fn refuses_an_event_holding_something_other_than_an_array() {
        let error = refusal("{\"hooks\": {\"PostToolUse\": {}}}\n", &[format_entry()]);

        assert_eq!(
            error.message,
            format!("\"hooks.PostToolUse\" in {FILE} is not a JSON array")
        );
        assert_eq!(
            error.detail,
            [
                "ambit appends one entry per managed hook to `hooks.PostToolUse`",
                "make `hooks.PostToolUse` an array, or move its current value aside",
            ]
        );
    }

    #[test]
    fn refuses_a_key_that_names_no_event_as_a_bug_rather_than_a_config_error() {
        let error = refusal(
            HANDWRITTEN,
            &[ConfigEntry {
                key: "PostToolUse".to_owned(),
                value: format_value(),
            }],
        );

        assert_eq!(error.code, ExitCode::Internal);
        assert_eq!(
            error.message,
            format!("cannot address \"PostToolUse\" in {FILE}")
        );
        assert_eq!(
            driver()
                .remove_keys(Some(HANDWRITTEN), SECTION, &["Stop".to_owned()], FILE)
                .expect_err("expected a refusal")
                .code,
            ExitCode::Internal
        );
    }
}
