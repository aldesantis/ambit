//! JSONC exists so that a person can annotate their config, so the claims here are the same ones
//! the TOML driver makes and for the same reason: comments, trailing commas, indentation and key
//! order everywhere ambit does not own survive being written into. What differs is drift: this
//! format *can* be parsed losslessly enough to compare values, so a reformatted entry is not a
//! change.

use indexmap::IndexSet;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::errors::{AmbitError, ExitCode};

const SECTION: &str = "mcp";
const FILE: &str = ".opencode/opencode.jsonc";

/// A local server, in the shape the opencode profile emits.
fn fixture() -> ConfigEntry {
    ConfigEntry {
        key: "fixture".to_owned(),
        value: json!({ "type": "local", "command": ["npx", "-y", "@acme/fixture-mcp"] }),
    }
}

fn entry(key: &str, value: JsonValue) -> ConfigEntry {
    ConfigEntry {
        key: key.to_owned(),
        value,
    }
}

/// A config someone maintains by hand: a line comment, a block comment, a trailing comma, and
/// keys ambit has no business touching.
const HANDWRITTEN: &str = r#"{
  // The model I actually use.
  "model": "anthropic/claude-opus-4",

  /* Servers I added myself, long before ambit ran here. */
  "mcp": {
    "handmade": {
      "type": "local",
      "command": ["node", "./scripts/local-mcp.js"],
    },
  },
}
"#;

fn merge(text: Option<&str>, entries: &[ConfigEntry]) -> String {
    JsoncDriver
        .merge_section(text, SECTION, entries, FILE)
        .expect("merge succeeds")
}

fn refusal(text: &str) -> AmbitError {
    JsoncDriver
        .merge_section(Some(text), SECTION, &[fixture()], FILE)
        .expect_err("expected a refusal")
}

/// The document a merge produced, parsed, for the claims that are about values, not bytes.
///
/// Through jsonc-parser rather than plain JSON, since the whole point of the fixtures here is that
/// they hold comments and trailing commas that plain JSON would reject.
fn parsed(text: &str) -> JsonObject {
    match jsonc_parser::parse_to_serde_value::<JsonValue>(text, &parse_options()) {
        Ok(JsonValue::Object(document)) => document,
        other => panic!("not a JSONC object: {other:?}\n{text}"),
    }
}

fn section_keys_of(text: &str) -> Vec<String> {
    parsed(text)[SECTION]
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect()
}

mod merging_a_server_into_an_opencode_config {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn keeps_every_comment_and_the_trailing_commas_around_it() {
        let merged = merge(Some(HANDWRITTEN), &[fixture()]);

        assert!(merged.contains("// The model I actually use."));
        assert!(merged.contains("/* Servers I added myself, long before ambit ran here. */"));
        // The trailing comma the person wrote is still there: this driver edits the syntax tree
        // rather than re-serializing the document, so it has no opinion about their style.
        assert!(merged.contains(r#""command": ["node", "./scripts/local-mcp.js"],"#));
    }

    #[test]
    fn adds_its_own_key_and_leaves_the_hand_written_one_in_place_in_order() {
        let merged = merge(Some(HANDWRITTEN), &[fixture()]);

        let document = parsed(&merged);

        assert_eq!(document["model"], json!("anthropic/claude-opus-4"));
        assert_eq!(section_keys_of(&merged), ["handmade", "fixture"]);
        assert_eq!(document[SECTION]["fixture"], fixture().value);
    }

    #[test]
    fn writes_a_whole_document_when_the_file_does_not_exist_yet() {
        assert_eq!(
            crate::util::json::parse(&merge(None, &[fixture()])).expect("plain JSON"),
            json!({ "mcp": { "fixture": fixture().value } })
        );
    }

    #[test]
    fn creates_the_section_in_a_document_that_has_none() {
        let merged = merge(
            Some("{\n  // just a model\n  \"model\": \"x\"\n}\n"),
            &[fixture()],
        );

        assert!(merged.contains("// just a model"));
        assert_eq!(
            JsonValue::Object(parsed(&merged)),
            json!({ "model": "x", "mcp": { "fixture": fixture().value } })
        );
    }

    #[test]
    fn merges_several_entries_each_against_the_text_the_last_one_produced() {
        let merged = merge(
            Some(HANDWRITTEN),
            &[
                fixture(),
                entry(
                    "other",
                    json!({ "type": "remote", "url": "https://other.invalid/mcp" }),
                ),
            ],
        );

        assert_eq!(section_keys_of(&merged), ["handmade", "fixture", "other"]);
    }

    #[test]
    fn is_idempotent_merging_what_the_file_already_says_changes_nothing() {
        let once = merge(Some(HANDWRITTEN), &[fixture()]);

        assert_eq!(merge(Some(&once), &[fixture()]), once);
    }

    #[test]
    fn replaces_an_entry_it_already_owns_rather_than_appending_a_second_one() {
        let once = merge(
            Some(HANDWRITTEN),
            &[entry(
                "fixture",
                json!({ "type": "local", "command": ["old"] }),
            )],
        );

        let merged = merge(Some(&once), &[fixture()]);

        assert_eq!(section_keys_of(&merged), ["handmade", "fixture"]);
        assert_eq!(parsed(&merged)[SECTION]["fixture"], fixture().value);
    }
}

mod removing_servers {
    use super::*;
    use pretty_assertions::assert_eq;

    fn remove(text: &str, keys: &[&str]) -> Option<String> {
        let keys: Vec<String> = keys.iter().map(|&key| key.to_owned()).collect();

        JsoncDriver
            .remove_keys(Some(text), SECTION, &keys, FILE)
            .expect("removal succeeds")
    }

    #[test]
    fn removes_the_key_and_keeps_the_comments_the_commas_and_the_foreign_keys() {
        let once = merge(Some(HANDWRITTEN), &[fixture()]);

        let removed = remove(&once, &["fixture"]).expect("something was removed");

        assert!(removed.contains("// The model I actually use."));
        assert!(removed.contains("/* Servers I added myself, long before ambit ran here. */"));
        assert_eq!(section_keys_of(&removed), ["handmade"]);
    }

    #[test]
    fn leaves_the_section_behind_when_it_empties_out() {
        let only = merge(Some("{\n  // mine\n  \"model\": \"x\"\n}\n"), &[fixture()]);

        let removed = remove(&only, &["fixture"]).expect("something was removed");

        // The section is a key ambit created but does not own the way it owns the entries in it,
        // and a person may be about to add their own server to it. `{}` is the honest state.
        assert_eq!(
            JsonValue::Object(parsed(&removed)),
            json!({ "model": "x", "mcp": {} })
        );
    }

    #[test]
    fn reports_nothing_to_do_rather_than_rewriting_a_file_it_would_not_change() {
        assert_eq!(remove(HANDWRITTEN, &["absent"]), None);
        assert_eq!(
            JsoncDriver.remove_keys(None, SECTION, &["fixture".to_owned()], FILE),
            Ok(None)
        );
        assert_eq!(remove("{\"model\": \"x\"}\n", &["fixture"]), None);
    }
}

mod reading_the_section {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn names_every_entry_through_comments_and_trailing_commas() {
        assert_eq!(
            JsoncDriver.section_keys(Some(HANDWRITTEN), SECTION, FILE),
            Ok(IndexSet::from(["handmade".to_owned()]))
        );
    }

    #[test]
    fn reads_an_absent_file_an_absent_section_and_a_non_object_section_as_holding_none() {
        // None of the three is a *collision* with anything ambit would write; an unusable section
        // is `merge_section`'s error to raise, since that is the code that cannot proceed with it.
        for text in [None, Some("{\"model\": \"x\"}"), Some("{\"mcp\": []}")] {
            assert_eq!(
                JsoncDriver.section_keys(text, SECTION, FILE),
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
        let merged = merge(Some(HANDWRITTEN), &[fixture()]);

        assert_eq!(
            JsoncDriver.entry_matches(Some(&merged), SECTION, &fixture(), FILE),
            Ok(true)
        );
    }

    #[test]
    fn says_yes_for_a_reformatted_reordered_entry_because_ambit_owns_the_value_and_not_its_layout()
    {
        // The opposite of the TOML driver's answer, deliberately: this format can be compared
        // structurally, so sending someone to look at a file that is already correct would be a
        // bug.
        let reformatted = r#"{
  "mcp": { "fixture": { "command": ["npx", "-y", "@acme/fixture-mcp"], "type": "local" } }
}
"#;

        assert_eq!(
            JsoncDriver.entry_matches(Some(reformatted), SECTION, &fixture(), FILE),
            Ok(true)
        );
    }

    #[test]
    fn says_no_for_an_absent_file_an_absent_entry_and_a_changed_one() {
        let changed = merge(
            Some(HANDWRITTEN),
            &[entry(
                "fixture",
                json!({ "type": "local", "command": ["old"] }),
            )],
        );

        for text in [None, Some(HANDWRITTEN), Some(changed.as_str())] {
            assert_eq!(
                JsoncDriver.entry_matches(text, SECTION, &fixture(), FILE),
                Ok(false)
            );
        }
    }
}

mod what_it_refuses {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn refuses_a_document_it_cannot_parse_even_tolerantly() {
        let error = refusal("{ not jsonc\n");

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(error.message, format!("{FILE} is not valid JSONC"));
        assert_eq!(
            error.detail[1],
            "correct the syntax, so ambit can add its own keys without discarding the rest"
        );
    }

    #[test]
    fn refuses_a_document_with_no_value_at_all() {
        for text in ["", "  \n", "// only a comment\n"] {
            let error = refusal(text);

            assert_eq!(error.message, format!("{FILE} is not valid JSONC"));
            assert_eq!(
                error.detail[0],
                format!("parse error at offset {}", text.len())
            );
        }
    }

    #[test]
    fn refuses_a_document_whose_root_is_not_an_object() {
        assert_eq!(
            refusal("[1, 2]\n").message,
            format!("{FILE} is not a JSONC object")
        );
    }

    #[test]
    fn refuses_a_section_holding_something_other_than_an_object() {
        let error = refusal("{\"mcp\": []}\n");

        assert_eq!(
            error.message,
            format!("\"mcp\" in {FILE} is not a JSONC object")
        );
        assert_eq!(
            error.detail,
            [
                "ambit writes one key per managed entry inside `mcp`",
                "make `mcp` an object, or move its current value aside",
            ]
        );
    }
}

/// Every input `jsonc.test.ts` fed the TypeScript driver, with the exact bytes this driver writes
/// for it.
///
/// Recorded from the TypeScript driver, then updated where jsonc-parser's formatting differs from
/// VS Code's `modify`; the PR description lists each difference.
#[test]
fn writes_the_recorded_bytes() {
    let corpus: JsonValue =
        serde_json::from_str(include_str!("../../../../tests/fixtures/jsonc_edits.json"))
            .expect("valid fixture");
    let section = corpus["section"].as_str().expect("a section");
    let file = corpus["file"].as_str().expect("a file");

    for case in corpus["cases"].as_array().expect("a cases array") {
        let text = case["text"].as_str();
        let result = match case["op"].as_str().expect("an op") {
            "mergeSection" => {
                let entries: Vec<ConfigEntry> = case["entries"]
                    .as_array()
                    .expect("entries")
                    .iter()
                    .map(|e| entry(e["key"].as_str().expect("a key"), e["value"].clone()))
                    .collect();

                JsoncDriver
                    .merge_section(text, section, &entries, file)
                    .map(|text| json!(text))
            }
            "removeKeys" => {
                let keys: Vec<String> = case["keys"]
                    .as_array()
                    .expect("keys")
                    .iter()
                    .map(|key| key.as_str().expect("a key").to_owned())
                    .collect();

                JsoncDriver
                    .remove_keys(text, section, &keys, file)
                    .map(|text| json!(text))
            }
            op => panic!("unknown op {op}"),
        };
        let actual = match result {
            Ok(text) => json!({ "ok": text }),
            Err(error) => json!({
                "error": {
                    "code": error.code.as_i32(),
                    "message": error.message,
                    "detail": error.detail,
                }
            }),
        };

        assert_eq!(actual, case["result"], "{case}");
    }
}
