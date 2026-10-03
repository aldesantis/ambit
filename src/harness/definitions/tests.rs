//! The five harness profiles, as a table of exact server shapes.
//!
//! This file is the specification for the property the whole harness layer rests on: an installed
//! config is indistinguishable from one a person wrote by hand. That is what makes a harness
//! willing to read the file and a person willing to look at it. So every case asserts the *whole*
//! emitted object rather than probing a field, and the key order too, since a hand-written server
//! does not put `url` before `type`.
//!
//! The layouts come from dotagents 1.19.0's own target definitions rather than from memory, with
//! one deliberate deviation for VS Code that is called out where it is asserted.
//!
//! Every comparison goes through [`same`], which compares the serialized bytes, so key order is
//! asserted along with the values.

use std::path::PathBuf;

use indexmap::IndexMap;
use serde_json::json;

use super::*;
use crate::harness::adapter::{
    HarnessAdapter, HookSkipReason, PlannedArtifact, ProjectPaths, SkippedHook,
};
use crate::harness::profile::{SHARED_SKILLS_DIR, adapter_for, skipped_hooks};
use crate::model::expectation::{Expectation, ExpectationKind};
use crate::model::hook_entity::{HOOK_EVENTS, HookType};
use crate::resolution::resolve::Bundle;
use crate::util::json::stringify;

/// Where Claude Code and Cursor read skills, and so the one link ambit plans.
const LINK: &str = ".claude/skills";

const URL: &str = "https://mcp.invalid/fixture";

/// Asserts two JSON values serialize to the same bytes: same values, same key order.
#[track_caller]
fn same(emitted: &JsonValue, expected: &JsonValue) {
    assert_eq!(stringify(emitted), stringify(expected));
}

fn pairs(list: &[(&str, &str)]) -> IndexMap<String, String> {
    list.iter()
        .map(|&(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

/// An http server carrying both header shapes: one embedded reference and one bare one.
fn http(headers: &[(&str, &str)], bearer_token_env_var: Option<&str>) -> MergedMcp {
    MergedMcp {
        name: "fixture".to_owned(),
        expects: Vec::new(),
        catalog: "company".to_owned(),
        file: "mcps/fixture.yml".to_owned(),
        transport: McpTransport::Http(HttpTransport {
            url: URL.to_owned(),
            bearer_token_env_var: bearer_token_env_var.map(str::to_owned),
            headers: pairs(headers),
        }),
    }
}

const HEADERS: &[(&str, &str)] = &[("X-Api-Key", "${API_KEY}")];

/// The same server at some other url, for the claims about the url itself.
fn http_at(at: &str) -> MergedMcp {
    MergedMcp {
        transport: McpTransport::Http(HttpTransport {
            url: at.to_owned(),
            bearer_token_env_var: None,
            headers: IndexMap::new(),
        }),
        ..http(&[], None)
    }
}

/// A stdio server whose arguments carry a credential, which is the case translation exists for.
fn stdio_server(args: &[&str], env: &[&str], declared: &[(&str, &str)]) -> MergedMcp {
    MergedMcp {
        name: "fixture".to_owned(),
        expects: env
            .iter()
            .map(|&name| Expectation {
                kind: ExpectationKind::Env,
                name: name.to_owned(),
            })
            .collect(),
        catalog: "company".to_owned(),
        file: "mcps/fixture.yml".to_owned(),
        transport: McpTransport::Stdio(StdioTransport {
            command: "npx".to_owned(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            env: pairs(declared),
        }),
    }
}

const BRIDGE_ARGS: &[&str] = &[
    "-y",
    "mcp-remote",
    URL,
    "--header",
    "Authorization: Bearer ${TOKEN}",
];

fn project(root: &str, scope: Option<InstallScope>) -> ProjectPaths {
    ProjectPaths {
        root: PathBuf::from(root),
        scope,
        mode: None,
    }
}

// the harness table

#[test]
fn ships_exactly_five_profiles_in_the_order_errors_and_help_list_them() {
    assert_eq!(
        PROFILES
            .iter()
            .map(|profile| profile.name)
            .collect::<Vec<_>>(),
        ["claude", "codex", "cursor", "opencode", "vscode"]
    );
}

#[test]
fn names_each_harnesss_config_file_section_and_format() {
    let layout = |file, user_file, section, format| McpLayout {
        file,
        user_file,
        section,
        format,
    };

    assert_eq!(
        PROFILES
            .iter()
            .map(|profile| (profile.name, profile.mcp.clone()))
            .collect::<Vec<_>>(),
        [
            (
                "claude",
                layout(
                    ".mcp.json",
                    Some(".claude.json"),
                    "mcpServers",
                    DocumentFormat::Json
                )
            ),
            (
                "codex",
                layout(
                    ".codex/config.toml",
                    None,
                    "mcp_servers",
                    DocumentFormat::Toml
                )
            ),
            (
                "cursor",
                layout(".cursor/mcp.json", None, "mcpServers", DocumentFormat::Json)
            ),
            (
                "opencode",
                layout(
                    ".opencode/opencode.jsonc",
                    None,
                    "mcp",
                    DocumentFormat::Jsonc
                )
            ),
            (
                "vscode",
                layout(".vscode/mcp.json", None, "servers", DocumentFormat::Json)
            ),
        ]
    );
}

#[test]
fn gives_the_two_harnesses_that_need_one_the_same_skills_link_and_the_other_three_none() {
    // Claude Code and Cursor read `.claude/skills`; Codex, VS Code and opencode read the shared
    // directory natively. Naming the same link is what makes a project using both plan it once.
    assert_eq!(CLAUDE.skills_link, Some(LINK));
    assert_eq!(CURSOR.skills_link, Some(LINK));
    assert_eq!(CODEX.skills_link, None);
    assert_eq!(VSCODE.skills_link, None);
    assert_eq!(OPENCODE.skills_link, None);
    // And the link is not the shared directory itself, or it would point at itself.
    assert_ne!(LINK, SHARED_SKILLS_DIR);
}

#[test]
fn writes_each_harnesss_config_where_that_harness_looks_for_a_project_local_one() {
    // Every path is project-relative and inside the project: ambit installs into a checkout, never
    // into someone's home directory.
    for profile in PROFILES.iter() {
        assert!(!profile.mcp.file.starts_with('/'));
        assert!(!profile.mcp.file.starts_with(".."));
    }
}

#[test]
fn writes_claude_user_mcps_to_the_user_config() {
    let bundle = Bundle {
        mcps: vec![http(&[], None)],
        ..Bundle::default()
    };
    let adapter = adapter_for(&CLAUDE);
    let config_path = |paths: &ProjectPaths| {
        adapter
            .plan(&bundle, paths)
            .into_iter()
            .find(|artifact| matches!(artifact, PlannedArtifact::HarnessConfig(_)))
            .map(|artifact| artifact.path().to_owned())
    };

    assert_eq!(
        config_path(&project("/home/jane", Some(InstallScope::User))).as_deref(),
        Some(".claude.json")
    );
    assert_eq!(
        config_path(&project("/home/jane/work", Some(InstallScope::Project))).as_deref(),
        Some(".mcp.json")
    );
}

// the server each profile emits
//
// One case per harness per transport, asserted whole. Table-driven because the interesting content
// is the table: reading down a column is how someone checks a harness's shape against that
// harness's documentation, and how the deviations between them stay visible rather than buried in
// five separate assertions.

struct Case {
    profile: &'static HarnessProfile,
    stdio: JsonValue,
    bare_stdio: JsonValue,
    http: JsonValue,
    bare_http: JsonValue,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            profile: &CLAUDE,
            // `type` is omitted for stdio, where `command` already says so, and emitted for http,
            // because Claude Code reads a server without one as stdio.
            stdio: json!({
                "command": "npx",
                "args": ["-y", "mcp-remote", URL, "--header", "Authorization: Bearer ${TOKEN}"],
                "env": { "FIXTURE_API_KEY": "${FIXTURE_API_KEY}", "TOKEN": "${TOKEN}" },
            }),
            bare_stdio: json!({ "command": "npx" }),
            http: json!({
                "type": "http",
                "url": URL,
                "headers": { "Authorization": "Bearer ${TOKEN}", "X-Api-Key": "${API_KEY}" },
            }),
            bare_http: json!({ "type": "http", "url": URL }),
        },
        Case {
            profile: &CURSOR,
            stdio: json!({
                "command": "npx",
                "args": ["-y", "mcp-remote", URL, "--header", "Authorization: Bearer ${TOKEN}"],
                "env": { "FIXTURE_API_KEY": "${FIXTURE_API_KEY}", "TOKEN": "${TOKEN}" },
            }),
            bare_stdio: json!({ "command": "npx" }),
            // No `type` at all: Cursor infers the transport from the presence of `url`.
            http: json!({
                "url": URL,
                "headers": {
                    "Authorization": "Bearer ${env:TOKEN}",
                    "X-Api-Key": "${env:API_KEY}",
                },
            }),
            bare_http: json!({ "url": URL }),
        },
        Case {
            profile: &VSCODE,
            // An explicit `type` on both transports, and `${env:VAR}` throughout, including in
            // `env`, where dotagents writes `${input:VAR}`. That form only works when the file
            // also declares a matching `inputs` array, which ambit does not write, so emitting it
            // would reference a prompt that does not exist.
            stdio: json!({
                "type": "stdio",
                "command": "npx",
                "args": ["-y", "mcp-remote", URL, "--header", "Authorization: Bearer ${env:TOKEN}"],
                "env": { "FIXTURE_API_KEY": "${env:FIXTURE_API_KEY}", "TOKEN": "${env:TOKEN}" },
            }),
            bare_stdio: json!({ "type": "stdio", "command": "npx" }),
            http: json!({
                "type": "http",
                "url": URL,
                "headers": {
                    "Authorization": "Bearer ${env:TOKEN}",
                    "X-Api-Key": "${env:API_KEY}",
                },
            }),
            bare_http: json!({ "type": "http", "url": URL }),
        },
        Case {
            profile: &CODEX,
            stdio: json!({
                "command": "npx",
                "args": ["-y", "mcp-remote", URL, "--header", "Authorization: Bearer ${TOKEN}"],
                "env": { "FIXTURE_API_KEY": "${FIXTURE_API_KEY}", "TOKEN": "${TOKEN}" },
            }),
            bare_stdio: json!({ "command": "npx" }),
            // Codex has first-class fields for bearer tokens and environment-backed header values.
            http: json!({
                "url": URL,
                "bearer_token_env_var": "TOKEN",
                "env_http_headers": { "X-Api-Key": "API_KEY" },
            }),
            bare_http: json!({ "url": URL }),
        },
        Case {
            profile: &OPENCODE,
            // Its own vocabulary throughout: `local`/`remote`, one `command` array, and
            // `environment`.
            stdio: json!({
                "type": "local",
                "command": ["npx", "-y", "mcp-remote", URL, "--header", "Authorization: Bearer ${TOKEN}"],
                "environment": { "FIXTURE_API_KEY": "${FIXTURE_API_KEY}", "TOKEN": "${TOKEN}" },
            }),
            bare_stdio: json!({ "type": "local", "command": ["npx"] }),
            http: json!({
                "type": "remote",
                "url": URL,
                "headers": { "Authorization": "Bearer {env:TOKEN}", "X-Api-Key": "{env:API_KEY}" },
            }),
            bare_http: json!({ "type": "remote", "url": URL }),
        },
    ]
}

#[test]
#[ignore = "needs B1: expected_env"]
fn writes_a_stdio_server_with_its_arguments_and_its_environment() {
    for case in cases() {
        let emitted = (case.profile.server_config)(&stdio_server(
            BRIDGE_ARGS,
            &["TOKEN", "FIXTURE_API_KEY"],
            &[],
        ));

        same(&emitted, &case.stdio);
    }
}

#[test]
#[ignore = "needs B1: expected_env"]
fn omits_args_and_env_a_stdio_server_does_not_declare() {
    // A server with nothing to say about either gets neither key, rather than an empty array and
    // an empty map nobody wrote.
    for case in cases() {
        same(
            &(case.profile.server_config)(&stdio_server(&[], &[], &[])),
            &case.bare_stdio,
        );
    }
}

#[test]
fn writes_an_http_server_with_its_headers() {
    for case in cases() {
        same(
            &(case.profile.server_config)(&http(HEADERS, Some("TOKEN"))),
            &case.http,
        );
    }
}

#[test]
fn omits_headers_an_http_server_does_not_declare() {
    for case in cases() {
        same(
            &(case.profile.server_config)(&http(&[], None)),
            &case.bare_http,
        );
    }
}

#[test]
fn sorts_the_headers_it_writes_so_the_file_does_not_churn_on_the_catalogs_key_order() {
    for case in cases() {
        let emitted = (case.profile.server_config)(&http(HEADERS, Some("TOKEN")));
        let written = ["headers", "http_headers", "env_http_headers"]
            .iter()
            .find_map(|key| emitted.get(key))
            .and_then(JsonValue::as_object)
            .expect("a header map");
        let keys: Vec<&String> = written.keys().collect();
        let mut sorted = keys.clone();

        sorted.sort();
        assert_eq!(keys, sorted, "{}", case.profile.name);
    }
}

#[test]
#[ignore = "needs B1: expected_env"]
fn resolves_no_variable_whatever_the_environment_holds() {
    // No profile takes an environment, so there is nothing a value could be resolved from; the TS
    // test stubbed `TOKEN` to prove the same.
    for case in cases() {
        let both = [
            stringify(&(case.profile.server_config)(&http(HEADERS, Some("TOKEN")))),
            stringify(&(case.profile.server_config)(&stdio_server(
                BRIDGE_ARGS,
                &["TOKEN"],
                &[],
            ))),
        ]
        .join("");

        assert!(!both.contains("s3cret"));
        assert!(both.contains("TOKEN"));
    }
}

// a reference in an http server's url

#[test]
fn is_translated_too_so_a_per_tenant_endpoint_works_on_every_harness() {
    let tenant = |profile: &HarnessProfile| {
        (profile.server_config)(&http_at("https://${TENANT}.mcp.invalid/fixture"))["url"].clone()
    };

    assert_eq!(tenant(&CLAUDE), "https://${TENANT}.mcp.invalid/fixture");
    assert_eq!(tenant(&CODEX), "https://${TENANT}.mcp.invalid/fixture");
    assert_eq!(tenant(&CURSOR), "https://${env:TENANT}.mcp.invalid/fixture");
    assert_eq!(tenant(&VSCODE), "https://${env:TENANT}.mcp.invalid/fixture");
    assert_eq!(
        tenant(&OPENCODE),
        "https://{env:TENANT}.mcp.invalid/fixture"
    );
}

// a stdio server whose env map renames a variable
//
// The env map an entity declares, per harness: the key is the name the spawned process reads, and
// the value is translated like any other reference. The two harnesses that spell a reference
// differently are what makes this worth asserting across the table rather than once.

const DECLARED: &[(&str, &str)] = &[("PLANNER_TOKEN", "${ACME_PLANNER_TOKEN}")];

fn env_of(profile: &HarnessProfile) -> JsonValue {
    let emitted = (profile.server_config)(&stdio_server(&[], &["ACME_PLANNER_TOKEN"], DECLARED));

    emitted
        .get("env")
        .or_else(|| emitted.get("environment"))
        .cloned()
        .unwrap_or(JsonValue::Null)
}

#[test]
#[ignore = "needs B1: expected_env"]
fn writes_the_declared_name_against_the_variable_that_supplies_it_in_every_spelling() {
    same(
        &env_of(&CLAUDE),
        &json!({ "PLANNER_TOKEN": "${ACME_PLANNER_TOKEN}" }),
    );
    same(
        &env_of(&CODEX),
        &json!({ "PLANNER_TOKEN": "${ACME_PLANNER_TOKEN}" }),
    );
    same(
        &env_of(&CURSOR),
        &json!({ "PLANNER_TOKEN": "${ACME_PLANNER_TOKEN}" }),
    );
    same(
        &env_of(&OPENCODE),
        &json!({ "PLANNER_TOKEN": "${ACME_PLANNER_TOKEN}" }),
    );
    same(
        &env_of(&VSCODE),
        &json!({ "PLANNER_TOKEN": "${env:ACME_PLANNER_TOKEN}" }),
    );
}

#[test]
#[ignore = "needs B1: expected_env"]
fn passes_an_expected_variable_no_entry_references_through_beside_it() {
    let emitted = (CLAUDE.server_config)(&stdio_server(
        &[],
        &["ACME_PLANNER_TOKEN", "PLANNER_WORKSPACE"],
        DECLARED,
    ));

    same(
        &emitted["env"],
        &json!({
            "PLANNER_TOKEN": "${ACME_PLANNER_TOKEN}",
            "PLANNER_WORKSPACE": "${PLANNER_WORKSPACE}",
        }),
    );
}

// a credential in a stdio server's arguments

#[test]
#[ignore = "needs B1: expected_env"]
fn reaches_every_harness_in_that_harnesss_own_spelling() {
    let args = ["mcp-remote", "--header", "Authorization: Bearer ${TOKEN}"];
    let last_arg = |profile: &HarnessProfile| {
        let emitted = (profile.server_config)(&stdio_server(&args, &[], &[]));
        let list = emitted
            .get("args")
            .or_else(|| emitted.get("command"))
            .and_then(JsonValue::as_array)
            .cloned()
            .unwrap_or_default();

        list.last().cloned().unwrap_or(JsonValue::Null)
    };

    assert_eq!(last_arg(&CLAUDE), "Authorization: Bearer ${TOKEN}");
    assert_eq!(last_arg(&CODEX), "Authorization: Bearer ${TOKEN}");
    assert_eq!(last_arg(&CURSOR), "Authorization: Bearer ${TOKEN}");
    assert_eq!(last_arg(&OPENCODE), "Authorization: Bearer ${TOKEN}");
    assert_eq!(last_arg(&VSCODE), "Authorization: Bearer ${env:TOKEN}");
}

// the hook each profile emits
//
// Four harnesses express hooks, in two shapes: Claude, VS Code and Codex render one entry (the
// first two into one shared file, Codex into its own) and Cursor shares nothing with any of them.
// So the claims are each entry's exact shape, its key order, and which harnesses render the same
// bytes. Key order is load-bearing here in a way it is not for a server: the managed key is a
// digest of these bytes, so reordering them renames every hook every project owns.
//
// The fifth harness expresses none, which is the other thing asserted here: opencode carries no
// layout, and `skipped_hooks` turns that absence into something the run reports.

fn plain_project() -> ProjectPaths {
    project("/tmp/ambit-project", None)
}

/// A hook carrying both optional fields, which is where the shape has anything to say.
fn hook() -> MergedHook {
    MergedHook {
        name: "block-rm".to_owned(),
        description: None,
        catalog: "company".to_owned(),
        path: "hooks/block-rm".to_owned(),
        catalog_root: PathBuf::from("/tmp/ambit-catalog"),
        commit: None,
        expects: Vec::new(),
        event: HookEvent::PreToolUse,
        matcher: Some("Bash".to_owned()),
        r#type: HookType::Command,
        command: "./bin/block-rm".to_owned(),
        timeout: Some(30),
    }
}

/// A hook carrying neither, on the event a `matcher` is not even allowed on.
fn bare() -> MergedHook {
    MergedHook {
        name: "greet".to_owned(),
        path: "hooks/greet".to_owned(),
        event: HookEvent::SessionStart,
        matcher: None,
        command: "./bin/greet".to_owned(),
        timeout: None,
        ..hook()
    }
}

fn render(profile: &HarnessProfile, hook: &MergedHook, paths: &ProjectPaths) -> JsonValue {
    (profile.hook_config.expect("a hook renderer"))(hook, paths)
}

/// A layout's fields, for comparison: `HookLayout` holds a function and so is not `PartialEq`.
fn layout_of(
    layout: &HookLayout,
) -> (
    &'static str,
    &'static str,
    DocumentFormat,
    DocumentShape,
    Option<String>,
    bool,
) {
    (
        layout.file,
        layout.section,
        layout.format,
        layout.shape,
        layout
            .root_defaults
            .as_ref()
            .map(|defaults| stringify(&JsonValue::Object(defaults.clone()))),
        layout.events.is_some(),
    )
}

#[test]
fn gives_claude_and_vscode_one_shared_file_and_codex_one_of_its_own() {
    let layout = (
        ".claude/settings.json",
        "hooks",
        DocumentFormat::Json,
        DocumentShape::Array,
        None,
        false,
    );

    assert_eq!(layout_of(CLAUDE.hooks.as_ref().unwrap()), layout);
    // The same file, so a project configuring both writes it once.
    assert_eq!(layout_of(VSCODE.hooks.as_ref().unwrap()), layout);
    // Codex differs in the file and in nothing else: Claude's section, Claude's shape, Claude's
    // entries. Not `[hooks]` in `.codex/config.toml`, which Codex also reads: that is an
    // array-of-tables, which the TOML driver refuses.
    assert_eq!(
        layout_of(CODEX.hooks.as_ref().unwrap()),
        (
            ".codex/hooks.json",
            "hooks",
            DocumentFormat::Json,
            DocumentShape::Array,
            None,
            false
        )
    );
    // No `events` on any of the three: all read ambit's own PascalCase spellings. And no
    // `root_defaults` either.
    assert!(OPENCODE.hooks.is_none());
}

#[test]
fn leaves_opencode_without_hooks_which_is_what_makes_a_hook_for_it_a_skip() {
    // The one harness with no declarative mechanism at all: it runs TypeScript plugins, which is
    // code rather than config. So the profile carries no layout and no renderer, and
    // `skipped_hooks` reads that absence as the reason.
    assert!(OPENCODE.hooks.is_none());
    assert!(OPENCODE.hook_config.is_none());
    assert_eq!(
        PROFILES
            .iter()
            .filter(|profile| profile.hooks.is_none())
            .map(|profile| profile.name)
            .collect::<Vec<_>>(),
        ["opencode"]
    );
}

#[test]
fn gives_cursor_a_file_of_its_own_a_version_to_seed_and_its_own_event_names() {
    let layout = CURSOR.hooks.as_ref().unwrap();

    assert_eq!(
        layout_of(layout),
        (
            ".cursor/hooks.json",
            "hooks",
            DocumentFormat::Json,
            DocumentShape::Array,
            Some(r#"{"version":1}"#.to_owned()),
            true
        )
    );

    let spell = layout.events.unwrap();

    assert_eq!(
        HOOK_EVENTS
            .iter()
            .map(|&event| (event.as_str(), spell(event)))
            .collect::<Vec<_>>(),
        [
            ("SessionStart", "sessionStart"),
            ("UserPromptSubmit", "userPromptSubmit"),
            ("PreToolUse", "preToolUse"),
            ("PostToolUse", "postToolUse"),
            ("Stop", "stop"),
            ("SubagentStop", "subagentStop"),
            ("PreCompact", "preCompact"),
            ("SessionEnd", "sessionEnd"),
        ]
    );
}

#[test]
fn pairs_a_layout_with_a_renderer_so_a_profile_carries_both_or_neither() {
    for profile in PROFILES.iter() {
        assert_eq!(
            profile.hook_config.is_none(),
            profile.hooks.is_none(),
            "{}",
            profile.name
        );
    }
}

#[test]
#[ignore = "needs B1: hook_command"]
fn writes_the_entry_claude_codes_own_documentation_describes_in_that_key_order() {
    same(
        &render(&CLAUDE, &hook(), &plain_project()),
        &json!({
            "matcher": "Bash",
            "hooks": [{ "type": "command", "command": "./bin/block-rm", "timeout": 30 }],
        }),
    );
}

#[test]
#[ignore = "needs B1: hook_command"]
fn omits_a_matcher_and_a_timeout_the_hook_does_not_declare() {
    same(
        &render(&CLAUDE, &bare(), &plain_project()),
        &json!({ "hooks": [{ "type": "command", "command": "./bin/greet" }] }),
    );
}

#[test]
#[ignore = "needs B1: hook_command"]
fn renders_one_entry_for_the_three_harnesses_that_read_claudes_shape() {
    // Byte equality, not structural: the digest that identifies the entry is taken over exactly
    // these bytes, so two renderings that differ only in key order would be two entries in one
    // array.
    let claudes = stringify(&render(&CLAUDE, &hook(), &plain_project()));

    assert_eq!(
        stringify(&render(&VSCODE, &hook(), &plain_project())),
        claudes
    );
    assert_eq!(
        stringify(&render(&CODEX, &hook(), &plain_project())),
        claudes
    );
}

#[test]
#[ignore = "needs B1: hook_command"]
fn writes_cursors_flat_entry_which_nests_nothing_and_carries_no_matcher() {
    // Cursor has no field for a tool `matcher`, so `Bash` is dropped rather than written through
    // into a key the harness would ignore, and no inner `hooks` array, because one entry is one
    // command.
    same(
        &render(&CURSOR, &hook(), &plain_project()),
        &json!({ "command": "./bin/block-rm", "timeout": 30 }),
    );
}

#[test]
#[ignore = "needs B1: hook_command"]
fn omits_a_timeout_a_cursor_hook_does_not_declare() {
    same(
        &render(&CURSOR, &bare(), &plain_project()),
        &json!({ "command": "./bin/greet" }),
    );
}

#[test]
#[ignore = "needs B1: hook_command"]
fn renders_cursors_entry_differently_from_claudes_which_is_why_the_files_stay_separate() {
    // Not a detail: the two renderings have different digests, so `plan_for` cannot collapse them
    // and a project on both harnesses gets two artifacts rather than one written twice.
    assert_ne!(
        stringify(&render(&CURSOR, &hook(), &plain_project())),
        stringify(&render(&CLAUDE, &hook(), &plain_project()))
    );
}

// a hook that ships its own script
//
// The one string that decides whether a materialized hook actually runs, and it is genuinely
// per-harness: a catalog declares `command: hook.sh`, which names a file relative to the hook's own
// directory in the catalog, a location no harness has ever heard of. So the rendered command has to
// say where the *installed* script is, spelled the way that harness resolves a path.
//
// Asserted as exact strings rather than by pattern, because a placeholder a harness does not
// interpolate is not a near miss: it is a hook that never fires, and it fails silently.

/// The script-shipping counterpart of [`hook`]: the same declaration, declared `script`.
fn script() -> MergedHook {
    MergedHook {
        catalog_root: PathBuf::from("/catalogs/company"),
        path: "hooks/block-rm".to_owned(),
        r#type: HookType::Script,
        command: "hook.sh".to_owned(),
        ..hook()
    }
}

/// The command out of one profile's rendering, whichever shape it wrote.
fn command_of(profile: &HarnessProfile, hook: &MergedHook, paths: &ProjectPaths) -> String {
    let emitted = render(profile, hook, paths);

    emitted
        .get("command")
        .or_else(|| {
            emitted
                .get("hooks")
                .and_then(|hooks| hooks[0].get("command"))
        })
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[test]
#[ignore = "needs B1: hook_command"]
fn points_claude_and_vscode_at_the_project_root_through_claudes_own_placeholder() {
    // `${CLAUDE_PROJECT_DIR}` is documented by Claude as interpolated in `command` and as holding
    // the project root. VS Code reads this same file and gets the same string, documented or not.
    assert_eq!(
        command_of(&CLAUDE, &script(), &plain_project()),
        "${CLAUDE_PROJECT_DIR}/.agents/hooks/block-rm/hook.sh"
    );
    assert_eq!(
        command_of(&VSCODE, &script(), &plain_project()),
        "${CLAUDE_PROJECT_DIR}/.agents/hooks/block-rm/hook.sh"
    );
}

#[test]
#[ignore = "needs B1: hook_command"]
fn writes_cursor_and_codex_a_project_relative_path_and_no_subshell() {
    // Neither interpolates anything in a `command`, so the path as written is all there is.
    assert_eq!(
        command_of(&CURSOR, &script(), &plain_project()),
        ".agents/hooks/block-rm/hook.sh"
    );
    assert_eq!(
        command_of(&CODEX, &script(), &plain_project()),
        ".agents/hooks/block-rm/hook.sh"
    );

    for profile in [&*CURSOR, &*CODEX] {
        assert!(!command_of(profile, &script(), &plain_project()).contains("rev-parse"));
        assert!(!command_of(profile, &script(), &plain_project()).contains("${"));
    }
}

#[test]
#[ignore = "needs B1: hook_command"]
fn gives_codex_a_different_command_from_claudes_sharing_the_entry_shape_and_not_the_path() {
    // Which is why `root` is a parameter of the Claude renderer rather than a constant inside it.
    assert_ne!(
        command_of(&CODEX, &script(), &plain_project()),
        command_of(&CLAUDE, &script(), &plain_project())
    );

    let keys = |profile: &HarnessProfile| {
        render(profile, &script(), &plain_project())
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };

    assert_eq!(keys(&CLAUDE), keys(&CODEX));
}

#[test]
#[ignore = "needs B1: hook_command"]
fn rewrites_the_program_and_keeps_every_argument() {
    // `command` is a shell fragment ambit does not parse, so only the first token is rewritten.
    let with_args = MergedHook {
        command: "./hook.sh --strict bin/other".to_owned(),
        ..script()
    };

    assert_eq!(
        command_of(&CLAUDE, &with_args, &plain_project()),
        "${CLAUDE_PROJECT_DIR}/.agents/hooks/block-rm/hook.sh --strict bin/other"
    );
    assert_eq!(
        command_of(&CURSOR, &with_args, &plain_project()),
        ".agents/hooks/block-rm/hook.sh --strict bin/other"
    );
}

#[test]
#[ignore = "needs B1: hook_command"]
fn leaves_a_hook_that_ships_nothing_exactly_as_declared() {
    // The command line case, which is most hooks: prefixing `npx --yes prettier` with a directory
    // would break it, and there are no bytes at that directory to point at anyway.
    let inline = MergedHook {
        command: "npx --yes prettier --check".to_owned(),
        ..hook()
    };

    for profile in [&*CLAUDE, &*CODEX, &*CURSOR, &*VSCODE] {
        assert_eq!(
            command_of(profile, &inline, &plain_project()),
            "npx --yes prettier --check",
            "{}",
            profile.name
        );
    }

    // Including one whose command reads exactly like a path and is still a command line: `type`
    // is the answer, and it was declared.
    assert_eq!(
        command_of(&CLAUDE, &hook(), &plain_project()),
        "./bin/block-rm"
    );
}

// at a user-level install
//
// A file under the home directory is the machine's config, read in every project. So the two
// spellings above stop being a way to say "the install root" and become a way to say "whatever
// project is open". Cloning a repository must not be enough to get code run, so the path is
// absolute here.

fn home() -> ProjectPaths {
    project("/home/jane", Some(InstallScope::User))
}

#[test]
#[ignore = "needs B1: hook_command"]
fn names_the_install_root_outright_for_every_harness_that_expresses_hooks() {
    for profile in [&*CLAUDE, &*CODEX, &*CURSOR, &*VSCODE] {
        assert_eq!(
            command_of(profile, &script(), &home()),
            "/home/jane/.agents/hooks/block-rm/hook.sh",
            "{}",
            profile.name
        );
    }
}

#[test]
#[ignore = "needs B1: hook_command"]
fn leaves_nothing_for_a_harness_or_a_project_to_resolve() {
    for profile in [&*CLAUDE, &*CODEX, &*CURSOR, &*VSCODE] {
        let command = command_of(profile, &script(), &home());

        // No placeholder, since `${CLAUDE_PROJECT_DIR}` is the project's root and not this one,
        // and nothing relative, which would resolve against the open project's tree.
        assert!(!command.contains("${"), "{}", profile.name);
        assert!(command.starts_with('/'), "{}", profile.name);
    }
}

#[test]
#[ignore = "needs B1: hook_command"]
fn still_rewrites_only_the_program_keeping_every_argument() {
    let with_args = MergedHook {
        command: "./hook.sh --strict bin/other".to_owned(),
        ..script()
    };

    assert_eq!(
        command_of(&CLAUDE, &with_args, &home()),
        "/home/jane/.agents/hooks/block-rm/hook.sh --strict bin/other"
    );
}

#[test]
#[ignore = "needs B1: hook_command"]
fn leaves_a_hook_that_ships_nothing_exactly_as_declared_at_a_user_level_install() {
    // Scope decides where a shipped script is, and a command line has no script to find.
    let inline = MergedHook {
        command: "npx --yes prettier --check".to_owned(),
        ..hook()
    };

    for profile in [&*CLAUDE, &*CODEX, &*CURSOR, &*VSCODE] {
        assert_eq!(
            command_of(profile, &inline, &home()),
            "npx --yes prettier --check",
            "{}",
            profile.name
        );
    }
}

#[test]
#[ignore = "needs B1: hook_command"]
fn resolves_no_variable_in_a_command_and_rewrites_no_reference_either() {
    // Unlike an MCP transport: a hook's command is run by a shell the harness spawns, so `${TOKEN}`
    // already means the right thing and translating it would be rewriting a shell fragment.
    let emitted = render(
        &CLAUDE,
        &MergedHook {
            command: "./bin/greet ${TOKEN}".to_owned(),
            ..bare()
        },
        &plain_project(),
    );

    assert!(stringify(&emitted).contains("${TOKEN}"));
    assert!(!stringify(&emitted).contains("s3cret"));
}

// skipped_hooks
//
// One predicate answers both (a hook is planned for the array it belongs in, or skipped because
// there is none), so these cases and the ones above partition every hook a bundle can hold.
//
// The TS case "skips a hook whose event a harness has no spelling for" is not ported: it built a
// profile whose `events` map was partial, and `HookLayout::events` is a total function here, so
// that profile cannot be written.

fn skip(harness: &str, hook: &str, event: HookEvent, reason: HookSkipReason) -> SkippedHook {
    SkippedHook {
        harness: harness.to_owned(),
        hook: hook.to_owned(),
        event,
        reason,
    }
}

#[test]
fn accounts_for_every_hook_on_the_harness_that_expresses_none() {
    assert_eq!(
        skipped_hooks(&OPENCODE, &[hook(), bare()]),
        [
            skip(
                "opencode",
                "block-rm",
                HookEvent::PreToolUse,
                HookSkipReason::NoMechanism
            ),
            skip(
                "opencode",
                "greet",
                HookEvent::SessionStart,
                HookSkipReason::NoMechanism
            ),
        ]
    );
}

#[test]
fn skips_nothing_on_the_four_harnesses_that_do_for_every_event_ambit_knows() {
    for profile in PROFILES.iter().filter(|profile| profile.hooks.is_some()) {
        let every: Vec<MergedHook> = HOOK_EVENTS
            .iter()
            .map(|&event| MergedHook { event, ..bare() })
            .collect();

        assert_eq!(skipped_hooks(profile, &every), [], "{}", profile.name);
    }
}
