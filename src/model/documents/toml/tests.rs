//! The TOML driver, which edits `.codex/config.toml` lexically.
//!
//! Every claim here is about *bytes*, because that is the whole reason this driver exists rather
//! than a TOML library: the file holds a person's model, sandbox and approval settings, usually
//! with comments explaining them, and a parse-and-stringify round trip loses all of it. So the
//! assertions compare whole documents rather than parsed values. A test that parsed the result
//! would pass while the comments went missing, which is precisely the bug this code is written to
//! avoid.
//!
//! The refusals are asserted just as hard. Working lexically means some legal TOML has no span to
//! replace, and for those the promised behavior is exit 2 with the file untouched, never a guess.

use indexmap::IndexSet;
use pretty_assertions::assert_eq;
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

/// A stdio server, in the shape the codex profile emits.
fn fixture() -> ConfigEntry {
    entry(
        "fixture",
        json!({ "command": "npx", "args": ["-y", "@acme/fixture-mcp"] }),
    )
}

/// One with a sub-table, which is how `env` and `http_headers` are rendered.
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

/// A config of the kind a person actually has: settings at the root, a foreign table, and comments
/// explaining both. None of it is ambit's, so none of it may move.
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

/// The error a refusal raises, so a test can assert its code and its wording together.
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
        // Stated separately from the byte comparison above, because "the comments survived" is the
        // claim this driver exists for and it should fail by name when it stops being true.
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
        // `env = { FIXTURE_API_KEY = "..." }` would be legal TOML and unreadable at three keys; it
        // is also a shape this driver could not later replace in place.
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

        // Appending a second server sees a file already ending in a table, so it adds one blank
        // line: not two, and not none.
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

        // The stale sub-table is gone rather than left behind next to the new table: ambit owns the
        // whole `[mcp_servers.fixture]` span, sub-tables included.
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

        // The comment is not part of the span (the span starts at the header), so a person's note
        // about the server survives ambit rewriting it.
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

        // There is no preceding separator to absorb here, so the following one is taken instead.
        // Otherwise the document would start with a blank line, and (since nothing else ever
        // rewrites those bytes) it would stay there for good.
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
        // `None` is what lets a prune skip the write entirely, which is what keeps a project with
        // nothing stale byte-identical, and what stops pruning from recreating a deleted file.
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
        // Written quoted and read back quoted, so the second install replaces the table rather than
        // appending a duplicate of it.
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
        // `"X-Api-Key" = ...` would also be legal, but nobody writes it that way by hand and
        // looking hand-written is the point.
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
        // Every line, including the ones this merge added: a file that mixed endings would confuse
        // both git and the next reader.
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
        // The refusals are confined to the managed section. A person's inline tables and dotted
        // keys elsewhere are none of ambit's business, and refusing them would make the driver
        // useless.
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

        // A profile handing this driver a string is a bug in ambit, not something wrong with the
        // file, and the message has to say so or someone will go looking at their own config.
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

/// Drift, for a format that cannot be compared structurally.
///
/// The other two drivers parse and compare values, so reformatting is not drift. TOML has no such
/// option here, so the question is answered the only other honest way (would a merge change the
/// file?) and a hand-reformatted table therefore *is* drift. That is a documented tradeoff, so it
/// is pinned rather than left as an accident.
mod whether_an_entry_is_already_what_install_would_write {
    use super::*;
    use pretty_assertions::assert_eq;

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
