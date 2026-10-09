use indexmap::IndexSet;
use serde_json::json;

use super::*;
use crate::model::documents::{DocumentFormat, DocumentShape, driver_for};
use crate::util::json::{parse, stringify_pretty};

const SECTION: &str = "hooks";
const FILE: &str = ".kiro/hooks/ambit.json";

fn driver() -> ListSectionDriver {
    list_section_driver(None)
}

fn versioned() -> ListSectionDriver {
    let defaults = json!({ "version": "v1" });

    list_section_driver(defaults.as_object())
}

fn guard_value() -> JsonValue {
    json!({
        "name": "guard",
        "trigger": "PreToolUse",
        "matcher": "shell",
        "action": { "type": "command", "command": "./guard.sh" },
        "timeout": 30,
    })
}

fn guard_entry() -> ConfigEntry {
    ConfigEntry {
        key: array_entry_key("PreToolUse", &guard_value()),
        value: guard_value(),
    }
}

fn greet_value() -> JsonValue {
    json!({
        "name": "greet",
        "trigger": "SessionStart",
        "action": { "type": "command", "command": "./greet.sh" },
    })
}

fn greet_entry() -> ConfigEntry {
    ConfigEntry {
        key: array_entry_key("SessionStart", &greet_value()),
        value: greet_value(),
    }
}

const HANDWRITTEN: &str = r#"{
  "version": "v1",
  "description": "mine",
  "hooks": [
    {
      "name": "lint",
      "trigger": "PreToolUse",
      "action": {
        "type": "command",
        "command": "./lint.sh"
      }
    }
  ]
}
"#;

fn mine() -> JsonValue {
    json!({
        "name": "lint",
        "trigger": "PreToolUse",
        "action": { "type": "command", "command": "./lint.sh" },
    })
}

fn merge(text: Option<&str>, entries: &[ConfigEntry]) -> String {
    driver()
        .merge_section(text, SECTION, entries, FILE)
        .expect("merge succeeds")
}

fn remove(driver: &ListSectionDriver, text: &str, keys: &[String]) -> String {
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

fn keys(object: &JsonObject) -> Vec<&str> {
    object.keys().map(String::as_str).collect()
}

mod merging_into_hand_written_hooks {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn appends_to_the_list_and_leaves_every_other_byte_identical() {
        assert_eq!(
            merge(Some(HANDWRITTEN), &[guard_entry()]),
            r#"{
  "version": "v1",
  "description": "mine",
  "hooks": [
    {
      "name": "lint",
      "trigger": "PreToolUse",
      "action": {
        "type": "command",
        "command": "./lint.sh"
      }
    },
    {
      "name": "guard",
      "trigger": "PreToolUse",
      "matcher": "shell",
      "action": {
        "type": "command",
        "command": "./guard.sh"
      },
      "timeout": 30
    }
  ]
}
"#
        );
    }

    #[test]
    fn writes_a_whole_document_when_the_file_does_not_exist_yet() {
        assert_eq!(
            merge(None, &[guard_entry(), greet_entry()]),
            format!(
                "{}\n",
                stringify_pretty(&json!({ "hooks": [guard_value(), greet_value()] }))
            )
        );
    }

    #[test]
    fn is_idempotent_a_second_merge_of_the_same_entries_is_a_no_op() {
        let once = merge(Some(HANDWRITTEN), &[guard_entry(), greet_entry()]);

        assert_eq!(merge(Some(&once), &[guard_entry(), greet_entry()]), once);
    }

    #[test]
    fn seeds_root_defaults_on_creation_and_never_overwrites_them() {
        assert_eq!(
            versioned().merge_section(None, SECTION, &[guard_entry()], FILE),
            Ok(format!(
                "{}\n",
                stringify_pretty(&json!({ "version": "v1", "hooks": [guard_value()] }))
            ))
        );

        let theirs = "{\n  \"version\": \"v2\",\n  \"hooks\": []\n}\n";
        let merged = versioned()
            .merge_section(Some(theirs), SECTION, &[guard_entry()], FILE)
            .expect("merge succeeds");

        assert_eq!(parsed(&merged)["version"], json!("v2"));
        assert_eq!(keys(&parsed(&merged)), ["version", "hooks"]);
    }
}

mod removing_entries {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn drops_only_the_matching_entries_and_leaves_the_foreign_one_in_place() {
        let once = merge(Some(HANDWRITTEN), &[guard_entry(), greet_entry()]);

        let removed = remove(&driver(), &once, &[guard_entry().key, greet_entry().key]);

        assert_eq!(removed, HANDWRITTEN);
    }

    #[test]
    fn leaves_an_emptied_list_behind_and_adds_no_defaults() {
        let once = merge(Some("{\n  \"hooks\": []\n}\n"), &[greet_entry()]);

        let removed = remove(&versioned(), &once, &[greet_entry().key]);

        assert_eq!(JsonValue::Object(parsed(&removed)), json!({ "hooks": [] }));
    }

    #[test]
    fn reports_nothing_to_do_rather_than_rewriting_a_file_it_would_not_change() {
        let driver = driver();
        let guard_key = [guard_entry().key];

        for text in [
            None,
            Some(HANDWRITTEN),
            Some("{\"version\": \"v1\"}\n"),
            Some("{\"hooks\": {}}\n"),
        ] {
            assert_eq!(
                driver.remove_keys(text, SECTION, &guard_key, FILE),
                Ok(None)
            );
        }
    }

    #[test]
    fn does_not_remove_an_entry_with_the_same_digest_under_another_trigger() {
        let other = ConfigEntry {
            key: array_entry_key("PostToolUse", &guard_value()),
            value: guard_value(),
        };
        let once = merge(None, &[guard_entry()]);

        assert_eq!(
            driver().remove_keys(Some(&once), SECTION, &[other.key], FILE),
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
            Ok([array_entry_key("PreToolUse", &mine())]
                .into_iter()
                .collect())
        );
    }

    #[test]
    fn names_ambits_entry_once_it_is_merged_in() {
        let merged = merge(Some(HANDWRITTEN), &[guard_entry()]);

        assert!(
            driver()
                .section_keys(Some(&merged), SECTION, FILE)
                .expect("readable")
                .contains(&guard_entry().key)
        );
        assert_eq!(
            driver().entry_matches(Some(&merged), SECTION, &guard_entry(), FILE),
            Ok(true)
        );
    }

    #[test]
    fn reads_an_absent_or_unusable_section_and_an_entry_with_no_trigger_as_holding_none() {
        for text in [
            None,
            Some("{\"version\": \"v1\"}"),
            Some("{\"hooks\": {}}"),
            Some("{\"hooks\": [{\"name\": \"x\"}, {\"trigger\": 1}, 2]}"),
        ] {
            assert_eq!(
                driver().section_keys(text, SECTION, FILE),
                Ok(IndexSet::new())
            );
        }
    }

    #[test]
    fn says_no_for_an_entry_edited_by_hand() {
        let edited = merge(None, &[guard_entry()]).replacen("./guard.sh", "./other.sh", 1);

        assert_eq!(
            driver().entry_matches(Some(&edited), SECTION, &guard_entry(), FILE),
            Ok(false)
        );
    }
}

mod selecting_the_driver {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn takes_the_list_shape_with_the_root_defaults_it_is_given() {
        let defaults = json!({ "version": "v1" });
        let list = driver_for(
            DocumentFormat::Json,
            DocumentShape::List,
            defaults.as_object(),
        )
        .expect("a driver");

        assert_eq!(
            list.merge_section(None, SECTION, &[guard_entry()], FILE),
            versioned().merge_section(None, SECTION, &[guard_entry()], FILE)
        );
    }

    #[test]
    fn refuses_a_format_with_no_list_section_driver() {
        for format in [DocumentFormat::Toml, DocumentFormat::Jsonc] {
            let Err(error) = driver_for(format, DocumentShape::List, None) else {
                panic!("expected a refusal for {format}");
            };

            assert_eq!(error.code, ExitCode::Internal);
            assert_eq!(
                error.message,
                format!("no {format} driver for a list-shaped section")
            );
        }
    }
}

mod what_it_refuses {
    use super::*;
    use pretty_assertions::assert_eq;

    fn refusal(text: Option<&str>, entries: &[ConfigEntry]) -> AmbitError {
        driver()
            .merge_section(text, SECTION, entries, FILE)
            .expect_err("expected a refusal")
    }

    #[test]
    fn refuses_a_section_holding_something_other_than_an_array() {
        let error = refusal(Some("{\"hooks\": {}}\n"), &[guard_entry()]);

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(
            error.message,
            format!("\"hooks\" in {FILE} is not a JSON array")
        );
    }

    #[test]
    fn refuses_a_key_that_names_no_event_as_a_bug() {
        let error = refusal(
            None,
            &[ConfigEntry {
                key: "PreToolUse".to_owned(),
                value: guard_value(),
            }],
        );

        assert_eq!(error.code, ExitCode::Internal);
        assert_eq!(
            driver()
                .remove_keys(Some(HANDWRITTEN), SECTION, &["Stop".to_owned()], FILE)
                .expect_err("expected a refusal")
                .code,
            ExitCode::Internal
        );
    }

    #[test]
    fn refuses_an_entry_whose_trigger_disagrees_with_its_key() {
        let error = refusal(
            None,
            &[ConfigEntry {
                key: array_entry_key("Stop", &guard_value()),
                value: guard_value(),
            }],
        );

        assert_eq!(error.code, ExitCode::Internal);
        assert_eq!(
            error.message,
            format!(
                "cannot address \"{}\" in {FILE}",
                array_entry_key("Stop", &guard_value())
            )
        );
    }
}
