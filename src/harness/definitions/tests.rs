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

const LINK: &str = ".claude/skills";

const URL: &str = "https://mcp.invalid/fixture";

#[track_caller]
fn same(emitted: &JsonValue, expected: &JsonValue) {
    assert_eq!(stringify(emitted), stringify(expected));
}

fn pairs(list: &[(&str, &str)]) -> IndexMap<String, String> {
    list.iter()
        .map(|&(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

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

#[test]
fn ships_every_profile_in_the_order_errors_and_help_list_them() {
    assert_eq!(
        PROFILES
            .iter()
            .map(|profile| profile.name)
            .collect::<Vec<_>>(),
        [
            "claude", "codex", "copilot", "cursor", "devin", "gemini", "grok", "kiro", "opencode"
        ]
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
                "copilot",
                layout(".vscode/mcp.json", None, "servers", DocumentFormat::Json)
            ),
            (
                "cursor",
                layout(".cursor/mcp.json", None, "mcpServers", DocumentFormat::Json)
            ),
            (
                "devin",
                layout(
                    ".devin/mcp_config.json",
                    None,
                    "mcpServers",
                    DocumentFormat::Json
                )
            ),
            (
                "gemini",
                layout(
                    ".gemini/settings.json",
                    None,
                    "mcpServers",
                    DocumentFormat::Json
                )
            ),
            (
                "grok",
                layout(
                    ".grok/config.toml",
                    None,
                    "mcp_servers",
                    DocumentFormat::Toml
                )
            ),
            (
                "kiro",
                layout(
                    ".kiro/settings/mcp.json",
                    None,
                    "mcpServers",
                    DocumentFormat::Json
                )
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
        ]
    );
}

#[test]
fn links_each_harness_that_does_not_read_the_shared_directory_and_no_other() {
    assert_eq!(CLAUDE.skills_link, Some(LINK));
    assert_eq!(CURSOR.skills_link, Some(LINK));
    assert_eq!(KIRO.skills_link, Some(".kiro/skills"));
    assert_eq!(GROK.skills_link, Some(".grok/skills"));
    assert_eq!(CODEX.skills_link, None);
    assert_eq!(COPILOT.skills_link, None);
    assert_eq!(DEVIN.skills_link, None);
    assert_eq!(GEMINI.skills_link, None);
    assert_eq!(OPENCODE.skills_link, None);
    assert_ne!(LINK, SHARED_SKILLS_DIR);
}

#[test]
fn writes_each_harnesss_config_where_that_harness_looks_for_a_project_local_one() {
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
            profile: &COPILOT,
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
            http: json!({
                "url": URL,
                "bearer_token_env_var": "TOKEN",
                "env_http_headers": { "X-Api-Key": "API_KEY" },
            }),
            bare_http: json!({ "url": URL }),
        },
        Case {
            profile: &GEMINI,
            stdio: json!({
                "command": "npx",
                "args": ["-y", "mcp-remote", URL, "--header", "Authorization: Bearer ${TOKEN}"],
                "env": { "FIXTURE_API_KEY": "${FIXTURE_API_KEY}", "TOKEN": "${TOKEN}" },
            }),
            bare_stdio: json!({ "command": "npx" }),
            http: json!({
                "httpUrl": URL,
                "headers": { "Authorization": "Bearer ${TOKEN}", "X-Api-Key": "${API_KEY}" },
            }),
            bare_http: json!({ "httpUrl": URL }),
        },
        Case {
            profile: &KIRO,
            stdio: json!({
                "command": "npx",
                "args": ["-y", "mcp-remote", URL, "--header", "Authorization: Bearer ${TOKEN}"],
                "env": { "FIXTURE_API_KEY": "${FIXTURE_API_KEY}", "TOKEN": "${TOKEN}" },
            }),
            bare_stdio: json!({ "command": "npx" }),
            http: json!({
                "url": URL,
                "headers": { "Authorization": "Bearer ${TOKEN}", "X-Api-Key": "${API_KEY}" },
            }),
            bare_http: json!({ "url": URL }),
        },
        Case {
            profile: &GROK,
            stdio: json!({
                "command": "npx",
                "args": ["-y", "mcp-remote", URL, "--header", "Authorization: Bearer ${TOKEN}"],
                "env": { "FIXTURE_API_KEY": "${FIXTURE_API_KEY}", "TOKEN": "${TOKEN}" },
            }),
            bare_stdio: json!({ "command": "npx" }),
            http: json!({
                "url": URL,
                "headers": { "Authorization": "Bearer ${TOKEN}", "X-Api-Key": "${API_KEY}" },
            }),
            bare_http: json!({ "url": URL }),
        },
        Case {
            profile: &DEVIN,
            stdio: json!({
                "command": "npx",
                "args": ["-y", "mcp-remote", URL, "--header", "Authorization: Bearer ${env:TOKEN}"],
                "env": { "FIXTURE_API_KEY": "${env:FIXTURE_API_KEY}", "TOKEN": "${env:TOKEN}" },
            }),
            bare_stdio: json!({ "command": "npx" }),
            http: json!({
                "url": URL,
                "transport": "http",
                "headers": {
                    "Authorization": "Bearer ${env:TOKEN}",
                    "X-Api-Key": "${env:API_KEY}",
                },
            }),
            bare_http: json!({ "url": URL, "transport": "http" }),
        },
        Case {
            profile: &OPENCODE,
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
fn omits_args_and_env_a_stdio_server_does_not_declare() {
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
fn resolves_no_variable_whatever_the_environment_holds() {
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

#[test]
fn is_translated_too_so_a_per_tenant_endpoint_works_on_every_harness() {
    let tenant = |profile: &HarnessProfile| {
        let emitted = (profile.server_config)(&http_at("https://${TENANT}.mcp.invalid/fixture"));

        emitted
            .get("url")
            .or_else(|| emitted.get("httpUrl"))
            .cloned()
            .unwrap_or(JsonValue::Null)
    };

    assert_eq!(tenant(&CLAUDE), "https://${TENANT}.mcp.invalid/fixture");
    assert_eq!(tenant(&CODEX), "https://${TENANT}.mcp.invalid/fixture");
    assert_eq!(tenant(&CURSOR), "https://${env:TENANT}.mcp.invalid/fixture");
    assert_eq!(
        tenant(&COPILOT),
        "https://${env:TENANT}.mcp.invalid/fixture"
    );
    assert_eq!(tenant(&DEVIN), "https://${env:TENANT}.mcp.invalid/fixture");
    assert_eq!(tenant(&GEMINI), "https://${TENANT}.mcp.invalid/fixture");
    assert_eq!(tenant(&GROK), "https://${TENANT}.mcp.invalid/fixture");
    assert_eq!(tenant(&KIRO), "https://${TENANT}.mcp.invalid/fixture");
    assert_eq!(
        tenant(&OPENCODE),
        "https://{env:TENANT}.mcp.invalid/fixture"
    );
}

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
        &env_of(&COPILOT),
        &json!({ "PLANNER_TOKEN": "${env:ACME_PLANNER_TOKEN}" }),
    );
    same(
        &env_of(&DEVIN),
        &json!({ "PLANNER_TOKEN": "${env:ACME_PLANNER_TOKEN}" }),
    );

    for profile in [&*GEMINI, &*GROK, &*KIRO] {
        same(
            &env_of(profile),
            &json!({ "PLANNER_TOKEN": "${ACME_PLANNER_TOKEN}" }),
        );
    }
}

#[test]
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

#[test]
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
    assert_eq!(last_arg(&COPILOT), "Authorization: Bearer ${env:TOKEN}");
    assert_eq!(last_arg(&DEVIN), "Authorization: Bearer ${env:TOKEN}");
    assert_eq!(last_arg(&GEMINI), "Authorization: Bearer ${TOKEN}");
    assert_eq!(last_arg(&GROK), "Authorization: Bearer ${TOKEN}");
    assert_eq!(last_arg(&KIRO), "Authorization: Bearer ${TOKEN}");
}

fn plain_project() -> ProjectPaths {
    project("/tmp/ambit-project", None)
}

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
fn gives_the_harnesses_reading_claudes_file_one_shared_file_and_codex_one_of_its_own() {
    let layout = (
        ".claude/settings.json",
        "hooks",
        DocumentFormat::Json,
        DocumentShape::Array,
        None,
        false,
    );

    assert_eq!(layout_of(CLAUDE.hooks.as_ref().unwrap()), layout);
    assert_eq!(layout_of(COPILOT.hooks.as_ref().unwrap()), layout);
    assert_eq!(layout_of(DEVIN.hooks.as_ref().unwrap()), layout);
    assert_eq!(layout_of(GROK.hooks.as_ref().unwrap()), layout);
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
    assert!(OPENCODE.hooks.is_none());
}

#[test]
fn leaves_opencode_without_hooks_which_is_what_makes_a_hook_for_it_a_skip() {
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
            ("SessionStart", Some("sessionStart")),
            ("UserPromptSubmit", Some("userPromptSubmit")),
            ("PreToolUse", Some("preToolUse")),
            ("PostToolUse", Some("postToolUse")),
            ("Stop", Some("stop")),
            ("SubagentStop", Some("subagentStop")),
            ("PreCompact", Some("preCompact")),
            ("SessionEnd", Some("sessionEnd")),
        ]
    );
}

#[test]
fn gives_gemini_its_settings_file_and_its_own_event_names() {
    let layout = GEMINI.hooks.as_ref().unwrap();

    assert_eq!(
        layout_of(layout),
        (
            ".gemini/settings.json",
            "hooks",
            DocumentFormat::Json,
            DocumentShape::Array,
            None,
            true
        )
    );
    assert_eq!(layout.file, GEMINI.mcp.file);
    assert_ne!(layout.section, GEMINI.mcp.section);

    let spell = layout.events.unwrap();

    assert_eq!(
        HOOK_EVENTS
            .iter()
            .map(|&event| (event.as_str(), spell(event)))
            .collect::<Vec<_>>(),
        [
            ("SessionStart", Some("SessionStart")),
            ("UserPromptSubmit", Some("BeforeAgent")),
            ("PreToolUse", Some("BeforeTool")),
            ("PostToolUse", Some("AfterTool")),
            ("Stop", Some("AfterAgent")),
            ("SubagentStop", None),
            ("PreCompact", Some("PreCompress")),
            ("SessionEnd", Some("SessionEnd")),
        ]
    );
}

#[test]
fn gives_kiro_a_file_of_ambits_own_holding_one_flat_list_and_a_version() {
    let layout = KIRO.hooks.as_ref().unwrap();

    assert_eq!(
        layout_of(layout),
        (
            ".kiro/hooks/ambit.json",
            "hooks",
            DocumentFormat::Json,
            DocumentShape::List,
            Some(r#"{"version":"v1"}"#.to_owned()),
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
            ("SessionStart", Some("SessionStart")),
            ("UserPromptSubmit", Some("UserPromptSubmit")),
            ("PreToolUse", Some("PreToolUse")),
            ("PostToolUse", Some("PostToolUse")),
            ("Stop", Some("Stop")),
            ("SubagentStop", None),
            ("PreCompact", None),
            ("SessionEnd", Some("SessionEnd")),
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
fn omits_a_matcher_and_a_timeout_the_hook_does_not_declare() {
    same(
        &render(&CLAUDE, &bare(), &plain_project()),
        &json!({ "hooks": [{ "type": "command", "command": "./bin/greet" }] }),
    );
}

#[test]
fn renders_one_entry_for_every_harness_that_reads_claudes_shape() {
    let claudes = stringify(&render(&CLAUDE, &hook(), &plain_project()));

    for profile in [&*COPILOT, &*CODEX, &*DEVIN, &*GROK] {
        assert_eq!(
            stringify(&render(profile, &hook(), &plain_project())),
            claudes,
            "{}",
            profile.name
        );
    }
}

#[test]
fn writes_geminis_entry_with_a_name_its_own_tool_names_and_a_timeout_in_milliseconds() {
    same(
        &render(&GEMINI, &hook(), &plain_project()),
        &json!({
            "matcher": "run_shell_command",
            "hooks": [{
                "name": "block-rm",
                "type": "command",
                "command": "./bin/block-rm",
                "timeout": 30000,
            }],
        }),
    );
    same(
        &render(&GEMINI, &bare(), &plain_project()),
        &json!({ "hooks": [{ "name": "greet", "type": "command", "command": "./bin/greet" }] }),
    );
}

#[test]
fn translates_each_claude_tool_name_in_a_matcher_and_leaves_everything_else_alone() {
    let matcher_of = |matcher: &str| {
        render(
            &GEMINI,
            &MergedHook {
                matcher: Some(matcher.to_owned()),
                ..hook()
            },
            &plain_project(),
        )["matcher"]
            .clone()
    };

    assert_eq!(matcher_of("Edit|Write"), "replace|write_file");
    assert_eq!(
        matcher_of("Read|Glob|Grep|LS"),
        "read_file|glob|grep_search|list_directory"
    );
    assert_eq!(
        matcher_of("WebFetch|WebSearch|TodoWrite"),
        "web_fetch|google_web_search|write_todos"
    );
    assert_eq!(
        matcher_of("mcp__github__.*|run_shell_command|Task|Bash.*"),
        "mcp__github__.*|run_shell_command|Task|Bash.*"
    );
}

#[test]
fn writes_kiros_flat_entry_naming_its_trigger_and_a_command_action() {
    same(
        &render(&KIRO, &hook(), &plain_project()),
        &json!({
            "name": "block-rm",
            "trigger": "PreToolUse",
            "matcher": "Bash",
            "action": { "type": "command", "command": "./bin/block-rm" },
            "timeout": 30,
        }),
    );
    same(
        &render(&KIRO, &bare(), &plain_project()),
        &json!({
            "name": "greet",
            "trigger": "SessionStart",
            "action": { "type": "command", "command": "./bin/greet" },
        }),
    );
}

#[test]
fn writes_cursors_flat_entry_which_nests_nothing_and_carries_no_matcher() {
    same(
        &render(&CURSOR, &hook(), &plain_project()),
        &json!({ "command": "./bin/block-rm", "timeout": 30 }),
    );
}

#[test]
fn omits_a_timeout_a_cursor_hook_does_not_declare() {
    same(
        &render(&CURSOR, &bare(), &plain_project()),
        &json!({ "command": "./bin/greet" }),
    );
}

#[test]
fn renders_cursors_entry_differently_from_claudes_which_is_why_the_files_stay_separate() {
    assert_ne!(
        stringify(&render(&CURSOR, &hook(), &plain_project())),
        stringify(&render(&CLAUDE, &hook(), &plain_project()))
    );
}

fn script() -> MergedHook {
    MergedHook {
        catalog_root: PathBuf::from("/catalogs/company"),
        path: "hooks/block-rm".to_owned(),
        r#type: HookType::Script,
        command: "hook.sh".to_owned(),
        ..hook()
    }
}

fn command_of(profile: &HarnessProfile, hook: &MergedHook, paths: &ProjectPaths) -> String {
    let emitted = render(profile, hook, paths);

    emitted
        .get("command")
        .or_else(|| {
            emitted
                .get("hooks")
                .and_then(|hooks| hooks[0].get("command"))
        })
        .or_else(|| {
            emitted
                .get("action")
                .and_then(|action| action.get("command"))
        })
        .and_then(JsonValue::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn points_claude_and_copilot_at_the_project_root_through_claudes_own_placeholder() {
    assert_eq!(
        command_of(&CLAUDE, &script(), &plain_project()),
        "${CLAUDE_PROJECT_DIR}/.agents/hooks/block-rm/hook.sh"
    );
    for profile in [&*COPILOT, &*DEVIN, &*GROK] {
        assert_eq!(
            command_of(profile, &script(), &plain_project()),
            "${CLAUDE_PROJECT_DIR}/.agents/hooks/block-rm/hook.sh",
            "{}",
            profile.name
        );
    }
}

#[test]
fn points_gemini_at_the_project_root_through_its_own_variable() {
    assert_eq!(
        command_of(&GEMINI, &script(), &plain_project()),
        "$GEMINI_PROJECT_DIR/.agents/hooks/block-rm/hook.sh"
    );
}

#[test]
fn writes_cursor_codex_and_kiro_a_project_relative_path_and_no_subshell() {
    assert_eq!(
        command_of(&KIRO, &script(), &plain_project()),
        ".agents/hooks/block-rm/hook.sh"
    );
    assert_eq!(
        command_of(&CURSOR, &script(), &plain_project()),
        ".agents/hooks/block-rm/hook.sh"
    );
    assert_eq!(
        command_of(&CODEX, &script(), &plain_project()),
        ".agents/hooks/block-rm/hook.sh"
    );

    for profile in [&*CURSOR, &*CODEX, &*KIRO] {
        assert!(!command_of(profile, &script(), &plain_project()).contains("rev-parse"));
        assert!(!command_of(profile, &script(), &plain_project()).contains("${"));
    }
}

#[test]
fn gives_codex_a_different_command_from_claudes_sharing_the_entry_shape_and_not_the_path() {
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
fn rewrites_the_program_and_keeps_every_argument() {
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
fn leaves_a_hook_that_ships_nothing_exactly_as_declared() {
    let inline = MergedHook {
        command: "npx --yes prettier --check".to_owned(),
        ..hook()
    };

    for profile in hook_profiles() {
        assert_eq!(
            command_of(profile, &inline, &plain_project()),
            "npx --yes prettier --check",
            "{}",
            profile.name
        );
    }

    assert_eq!(
        command_of(&CLAUDE, &hook(), &plain_project()),
        "./bin/block-rm"
    );
}

fn hook_profiles() -> impl Iterator<Item = &'static HarnessProfile> {
    PROFILES
        .iter()
        .copied()
        .filter(|profile| profile.hooks.is_some())
}

fn home() -> ProjectPaths {
    project("/home/jane", Some(InstallScope::User))
}

#[test]
fn names_the_install_root_outright_for_every_harness_that_expresses_hooks() {
    for profile in hook_profiles() {
        assert_eq!(
            command_of(profile, &script(), &home()),
            "/home/jane/.agents/hooks/block-rm/hook.sh",
            "{}",
            profile.name
        );
    }
}

#[test]
fn leaves_nothing_for_a_harness_or_a_project_to_resolve() {
    for profile in hook_profiles() {
        let command = command_of(profile, &script(), &home());

        assert!(!command.contains("${"), "{}", profile.name);
        assert!(command.starts_with('/'), "{}", profile.name);
    }
}

#[test]
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
fn leaves_a_hook_that_ships_nothing_exactly_as_declared_at_a_user_level_install() {
    let inline = MergedHook {
        command: "npx --yes prettier --check".to_owned(),
        ..hook()
    };

    for profile in hook_profiles() {
        assert_eq!(
            command_of(profile, &inline, &home()),
            "npx --yes prettier --check",
            "{}",
            profile.name
        );
    }
}

#[test]
fn resolves_no_variable_in_a_command_and_rewrites_no_reference_either() {
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

fn every_event() -> Vec<MergedHook> {
    HOOK_EVENTS
        .iter()
        .map(|&event| MergedHook { event, ..bare() })
        .collect()
}

#[test]
fn skips_nothing_on_the_harnesses_with_a_spelling_for_every_event() {
    for profile in [&*CLAUDE, &*CODEX, &*COPILOT, &*CURSOR, &*DEVIN, &*GROK] {
        assert_eq!(
            skipped_hooks(profile, &every_event()),
            [],
            "{}",
            profile.name
        );
    }
}

#[test]
fn skips_only_the_events_gemini_and_kiro_have_no_counterpart_for() {
    assert_eq!(
        skipped_hooks(&GEMINI, &every_event()),
        [skip(
            "gemini",
            "greet",
            HookEvent::SubagentStop,
            HookSkipReason::NoEvent
        )]
    );
    assert_eq!(
        skipped_hooks(&KIRO, &every_event()),
        [
            skip(
                "kiro",
                "greet",
                HookEvent::SubagentStop,
                HookSkipReason::NoEvent
            ),
            skip(
                "kiro",
                "greet",
                HookEvent::PreCompact,
                HookSkipReason::NoEvent
            ),
        ]
    );
}

#[test]
fn plans_no_script_and_no_entry_for_a_hook_it_skips() {
    let skipped = MergedHook {
        event: HookEvent::SubagentStop,
        ..script()
    };
    let bundle = Bundle {
        hooks: vec![skipped],
        ..Bundle::default()
    };

    for profile in [&*GEMINI, &*KIRO] {
        assert_eq!(
            adapter_for(profile).plan(&bundle, &plain_project()),
            [],
            "{}",
            profile.name
        );
    }
}
