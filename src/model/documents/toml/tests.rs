use indexmap::IndexSet;
use serde_json::json;

use super::*;
use crate::errors::{AmbitError, ExitCode};

const SECTION: &str = "mcp_servers";
const FILE: &str = ".codex/config.toml";

fn entry(key: &str, value: JsonValue) -> ConfigEntry {
    ConfigEntry {
        key: key.to_owned(),
        value,
    }
}

fn fixture() -> ConfigEntry {
    entry(
        "fixture",
        json!({ "command": "npx", "args": ["-y", "@acme/fixture-mcp"] }),
    )
}

fn with_env() -> ConfigEntry {
    entry(
        "fixture",
        json!({
            "command": "npx",
            "args": ["-y", "@acme/fixture-mcp"],
            "env": { "FIXTURE_API_KEY": "${FIXTURE_API_KEY}" },
        }),
    )
}

const HANDWRITTEN: &str = r#"# My Codex config. Comments here are load-bearing — they say why.
model = "gpt-5-codex"

# Read-only until I say otherwise.
[sandbox]
mode = "read-only"
"#;

fn merge(text: Option<&str>, entries: &[ConfigEntry]) -> String {
    TomlDriver
        .merge_section(text, SECTION, entries, FILE)
        .expect("merge succeeds")
}

fn remove(text: Option<&str>, keys: &[&str]) -> Option<String> {
    let keys: Vec<String> = keys.iter().map(|&key| key.to_owned()).collect();

    TomlDriver
        .remove_keys(text, SECTION, &keys, FILE)
        .expect("removal succeeds")
}

fn section_keys(text: Option<&str>) -> IndexSet<String> {
    TomlDriver
        .section_keys(text, SECTION, FILE)
        .expect("readable")
}

fn set(items: &[&str]) -> IndexSet<String> {
    items.iter().map(|&item| item.to_owned()).collect()
}

fn refusal(text: &str) -> AmbitError {
    TomlDriver
        .merge_section(Some(text), SECTION, &[fixture()], FILE)
        .expect_err("expected a refusal, but the merge succeeded")
}

mod merging_a_server_into_a_codex_config {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn appends_the_table_leaving_every_other_byte_of_the_file_identical() {
        let merged = merge(Some(HANDWRITTEN), &[fixture()]);

        assert_eq!(
            merged,
            format!(
                "{HANDWRITTEN}\n[mcp_servers.fixture]\ncommand = \"npx\"\nargs = [\"-y\", \"@acme/fixture-mcp\"]\n"
            )
        );
        assert!(
            merged.contains("# My Codex config. Comments here are load-bearing — they say why.")
        );
        assert!(merged.contains("# Read-only until I say otherwise."));
    }

    #[test]
    fn writes_a_whole_document_when_the_file_does_not_exist_yet() {
        assert_eq!(
            merge(None, &[fixture()]),
            r#"[mcp_servers.fixture]
command = "npx"
args = ["-y", "@acme/fixture-mcp"]
"#
        );
    }

    #[test]
    fn renders_a_nested_object_as_a_sub_table_rather_than_an_inline_one() {
        assert_eq!(
            merge(None, &[with_env()]),
            r#"[mcp_servers.fixture]
command = "npx"
args = ["-y", "@acme/fixture-mcp"]

[mcp_servers.fixture.env]
FIXTURE_API_KEY = "${FIXTURE_API_KEY}"
"#
        );
    }

    #[test]
    fn separates_an_appended_table_from_what_precedes_it_and_does_not_double_the_separator() {
        let once = merge(Some(HANDWRITTEN), &[fixture()]);

        assert_eq!(
            merge(
                Some(&once),
                &[entry("other", json!({ "command": "other-mcp" }))]
            ),
            format!("{once}\n[mcp_servers.other]\ncommand = \"other-mcp\"\n")
        );
    }

    #[test]
    fn appends_every_entry_it_is_given_in_the_order_it_is_given_them() {
        let merged = merge(
            None,
            &[
                entry("alpha", json!({ "command": "a" })),
                entry("beta", json!({ "command": "b" })),
            ],
        );

        assert_eq!(
            merged,
            r#"[mcp_servers.alpha]
command = "a"

[mcp_servers.beta]
command = "b"
"#
        );
    }

    #[test]
    fn is_idempotent_merging_what_the_file_already_says_changes_nothing() {
        let once = merge(Some(HANDWRITTEN), &[with_env()]);

        assert_eq!(merge(Some(&once), &[with_env()]), once);
    }
}

mod replacing_a_server_already_in_the_file {
    use super::*;
    use pretty_assertions::assert_eq;

    const BEFORE: &str = r#"# top matter
model = "gpt-5-codex"

[mcp_servers.fixture]
command = "old-command"

[mcp_servers.other]
command = "keep"

# A table after the servers, which must stay after them.
[sandbox]
mode = "read-only"
"#;

    #[test]
    fn replaces_the_table_in_place_keeping_its_position_and_its_neighbours() {
        assert_eq!(
            merge(Some(BEFORE), &[fixture()]),
            r#"# top matter
model = "gpt-5-codex"

[mcp_servers.fixture]
command = "npx"
args = ["-y", "@acme/fixture-mcp"]

[mcp_servers.other]
command = "keep"

# A table after the servers, which must stay after them.
[sandbox]
mode = "read-only"
"#
        );
    }

    #[test]
    fn replaces_a_servers_sub_tables_along_with_it_and_stops_at_the_next_server() {
        let before = r#"[mcp_servers.fixture]
command = "old-command"

[mcp_servers.fixture.env]
STALE = "${STALE}"

[mcp_servers.other]
command = "keep"
"#;

        assert_eq!(
            merge(Some(before), &[fixture()]),
            r#"[mcp_servers.fixture]
command = "npx"
args = ["-y", "@acme/fixture-mcp"]

[mcp_servers.other]
command = "keep"
"#
        );
    }

    #[test]
    fn does_not_swallow_the_blank_line_before_whatever_follows_the_table() {
        let before = r#"[mcp_servers.fixture]
command = "old-command"

[sandbox]
mode = "read-only"
"#;

        assert_eq!(
            merge(
                Some(before),
                &[entry("fixture", json!({ "command": "new-command" }))]
            ),
            r#"[mcp_servers.fixture]
command = "new-command"

[sandbox]
mode = "read-only"
"#
        );
    }

    #[test]
    fn leaves_a_comment_sitting_above_the_table_where_it_was() {
        let before = r#"# Why this server is here.
[mcp_servers.fixture]
command = "old-command"
"#;

        assert_eq!(
            merge(
                Some(before),
                &[entry("fixture", json!({ "command": "new-command" }))]
            ),
            r#"# Why this server is here.
[mcp_servers.fixture]
command = "new-command"
"#
        );
    }
}

mod removing_servers {
    use super::*;
    use pretty_assertions::assert_eq;

    const BOTH: &str = r#"# top matter

[mcp_servers.fixture]
command = "npx"

[mcp_servers.other]
command = "keep"
"#;

    #[test]
    fn removes_the_table_and_the_blank_line_that_separated_it() {
        assert_eq!(
            remove(Some(BOTH), &["fixture"]).as_deref(),
            Some(
                r#"# top matter

[mcp_servers.other]
command = "keep"
"#
            )
        );
    }

    #[test]
    fn removes_a_servers_sub_tables_with_it() {
        let text = r#"[mcp_servers.fixture]
command = "npx"

[mcp_servers.fixture.env]
FIXTURE_API_KEY = "${FIXTURE_API_KEY}"

[sandbox]
mode = "read-only"
"#;

        assert_eq!(
            remove(Some(text), &["fixture"]).as_deref(),
            Some("[sandbox]\nmode = \"read-only\"\n")
        );
    }

    #[test]
    fn leaves_no_blank_line_behind_when_the_table_it_removed_was_the_first_thing_in_the_file() {
        let servers = r#"[mcp_servers.fixture]
command = "npx"

[mcp_servers.other]
command = "keep"
"#;

        assert_eq!(
            remove(Some(servers), &["fixture"]).as_deref(),
            Some("[mcp_servers.other]\ncommand = \"keep\"\n")
        );
    }

    #[test]
    fn leaves_the_servers_it_was_not_asked_about_and_everything_else_in_the_file() {
        let removed = remove(Some(BOTH), &["fixture"]).expect("something was removed");

        assert_eq!(section_keys(Some(&removed)), set(&["other"]));
        assert!(removed.contains("# top matter"));
    }

    #[test]
    fn reports_nothing_to_do_rather_than_rewriting_a_file_it_would_not_change() {
        assert_eq!(remove(Some(BOTH), &["absent"]), None);
        assert_eq!(remove(None, &["fixture"]), None);
    }

    #[test]
    fn leaves_an_empty_document_rather_than_inventing_a_rewrite_of_the_rest() {
        let only = "[mcp_servers.fixture]\ncommand = \"npx\"\n";

        assert_eq!(remove(Some(only), &["fixture"]).as_deref(), Some(""));
    }
}

mod the_keys_and_strings_it_writes {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn quotes_a_server_name_that_is_not_a_bare_key_and_finds_it_again_afterwards() {
        let merged = merge(None, &[entry("acme.fixture", json!({ "command": "npx" }))]);

        assert_eq!(
            merged,
            "[mcp_servers.\"acme.fixture\"]\ncommand = \"npx\"\n"
        );
        assert_eq!(section_keys(Some(&merged)), set(&["acme.fixture"]));
        assert_eq!(
            merge(
                Some(&merged),
                &[entry("acme.fixture", json!({ "command": "npx" }))]
            ),
            merged
        );
    }

    #[test]
    fn leaves_a_hyphenated_header_key_bare_since_toml_allows_it() {
        assert_eq!(
            merge(
                None,
                &[entry(
                    "fixture",
                    json!({ "http_headers": { "X-Api-Key": "${KEY}" } })
                )]
            ),
            r#"[mcp_servers.fixture]

[mcp_servers.fixture.http_headers]
X-Api-Key = "${KEY}"
"#
        );
    }

    #[test]
    fn quotes_a_sub_table_key_toml_would_not_accept_bare() {
        assert_eq!(
            merge(
                None,
                &[entry(
                    "fixture",
                    json!({ "env": { "not a bare key": "${KEY}" } })
                )]
            ),
            r#"[mcp_servers.fixture]

[mcp_servers.fixture.env]
"not a bare key" = "${KEY}"
"#
        );
    }

    #[test]
    fn escapes_what_a_toml_basic_string_cannot_hold_literally() {
        let merged = merge(
            None,
            &[entry("fixture", json!({ "command": "a\"b\\c\td\ne" }))],
        );

        assert_eq!(
            merged,
            r#"[mcp_servers.fixture]
command = "a\"b\\c\td\ne"
"#
        );
    }

    #[test]
    fn renders_booleans_numbers_and_arrays_without_quoting_them() {
        assert_eq!(
            merge(
                None,
                &[entry(
                    "fixture",
                    json!({ "enabled": true, "timeout": 30, "args": ["-y", "pkg"] })
                )]
            ),
            r#"[mcp_servers.fixture]
enabled = true
timeout = 30
args = ["-y", "pkg"]
"#
        );
    }

    #[test]
    fn preserves_crlf_line_endings() {
        let crlf = HANDWRITTEN.replace('\n', "\r\n");

        let merged = merge(Some(&crlf), &[fixture()]);

        assert!(merged.starts_with(&crlf));
        assert_eq!(merged.split("\r\n").count(), merged.split('\n').count());
    }
}

mod what_it_refuses_to_edit {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn refuses_a_server_declared_as_an_inline_table_under_mcp_servers() {
        let error = refusal("[mcp_servers]\nfixture = { command = \"npx\" }\n");

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(
            error.message,
            format!("cannot edit \"mcp_servers\" in {FILE}")
        );
        assert_eq!(
            error.detail,
            [
                "line 2 sets `mcp_servers.fixture` outside a `[mcp_servers.<name>]` table",
                "rewrite it as a `[mcp_servers.<name>]` table, or move the file aside",
            ]
        );
    }

    #[test]
    fn refuses_a_dotted_key_under_mcp_servers() {
        assert_eq!(
            refusal("[mcp_servers]\nfixture.command = \"npx\"\n").detail[0],
            "line 2 sets `mcp_servers.fixture.command` outside a `[mcp_servers.<name>]` table"
        );
    }

    #[test]
    fn refuses_a_dotted_key_at_the_document_root() {
        assert_eq!(
            refusal("model = \"gpt-5-codex\"\nmcp_servers.fixture = { command = \"npx\" }\n")
                .detail[0],
            "line 2 sets `mcp_servers.fixture` outside a `[mcp_servers.<name>]` table"
        );
    }

    #[test]
    fn refuses_an_array_of_tables() {
        let error = refusal("[[mcp_servers.fixture]]\ncommand = \"npx\"\n");

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(
            error.detail,
            [
                "line 1 declares `mcp_servers` as an array of tables",
                "rewrite it as `[mcp_servers.<name>]` tables, or move the file aside",
            ]
        );
    }

    #[test]
    fn edits_a_file_whose_other_tables_use_the_shapes_it_refuses_for_its_own() {
        let text = r#"profile = { name = "default" }
tools.web_search = true

[[history.entries]]
id = 1
"#;

        assert_eq!(
            merge(Some(text), &[fixture()]),
            format!(
                "{text}\n[mcp_servers.fixture]\ncommand = \"npx\"\nargs = [\"-y\", \"@acme/fixture-mcp\"]\n"
            )
        );
    }
}

mod what_it_refuses_to_render {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn names_itself_as_the_culprit_for_an_entry_that_is_not_a_table() {
        let error = TomlDriver
            .merge_section(None, SECTION, &[entry("fixture", json!("npx"))], FILE)
            .expect_err("expected a refusal");

        assert_eq!(
            error.message,
            format!("cannot render \"mcp_servers.fixture\" for {FILE} as TOML")
        );
        assert_eq!(
            error.detail,
            [
                "a managed entry must be a table of keys",
                "this is a bug in ambit; please report it",
            ]
        );
    }

    #[test]
    fn says_the_same_for_a_value_type_toml_has_no_scalar_for() {
        let error = TomlDriver
            .merge_section(
                None,
                SECTION,
                &[entry("fixture", json!({ "command": null }))],
                FILE,
            )
            .expect_err("expected a refusal");

        assert_eq!(
            error.message,
            format!("cannot render a value for {FILE} as TOML")
        );
        assert_eq!(
            error.detail,
            [
                "unsupported value type: null",
                "this is a bug in ambit; please report it",
            ]
        );
    }
}

mod reading_the_section {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn names_every_server_table_and_nothing_else_in_the_file() {
        let text = format!(
            r#"{HANDWRITTEN}
[mcp_servers.fixture]
command = "npx"

[mcp_servers.fixture.env]
KEY = "${{KEY}}"

[mcp_servers."acme.other"]
command = "other"
"#
        );

        assert_eq!(section_keys(Some(&text)), set(&["fixture", "acme.other"]));
    }

    #[test]
    fn reads_an_absent_file_and_a_file_with_no_servers_as_holding_none() {
        assert_eq!(section_keys(None), IndexSet::new());
        assert_eq!(section_keys(Some(HANDWRITTEN)), IndexSet::new());
    }
}

mod whether_an_entry_is_already_what_install_would_write {
    use super::*;

    fn matches(text: Option<&str>, entry: &ConfigEntry) -> bool {
        TomlDriver
            .entry_matches(text, SECTION, entry, FILE)
            .expect("readable")
    }

    #[test]
    fn says_yes_for_the_table_install_wrote() {
        let text = merge(Some(HANDWRITTEN), &[with_env()]);

        assert!(matches(Some(&text), &with_env()));
    }

    #[test]
    fn says_no_for_an_absent_file_an_absent_table_and_a_changed_one() {
        assert!(!matches(None, &fixture()));
        assert!(!matches(Some(HANDWRITTEN), &fixture()));
        assert!(!matches(
            Some(&merge(Some(HANDWRITTEN), &[fixture()])),
            &with_env()
        ));
    }

    #[test]
    fn reads_a_reformatted_table_as_drift_which_is_the_accepted_cost_of_working_lexically() {
        let reformatted = r#"[mcp_servers.fixture]
args = ["-y", "@acme/fixture-mcp"]
command = "npx"
"#;

        assert!(!matches(Some(reformatted), &fixture()));
    }
}
