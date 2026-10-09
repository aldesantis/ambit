use std::sync::LazyLock;

use indexmap::IndexMap;
use serde_json::json;

use crate::harness::adapter::{InstallScope, ProjectPaths};
use crate::harness::env::{
    EnvRefStyle, braced_ref, namespaced_ref, shell_ref, sole_reference, stdio_env, translate_refs,
};
use crate::harness::profile::{HarnessProfile, HookLayout, McpLayout, SHARED_HOOKS_DIR};
use crate::model::catalog::{MergedHook, MergedMcp, hook_command};
use crate::model::documents::{DocumentFormat, DocumentShape, LIST_EVENT_FIELD};
use crate::model::expectation::expected_env;
use crate::model::hook_entity::HookEvent;
use crate::model::mcp_entity::{HttpTransport, McpTransport, StdioTransport};
use crate::util::cmp::js_cmp;
use crate::util::json::{JsonObject, JsonValue};

const CLAUDE_SKILLS_LINK: &str = ".claude/skills";

const KIRO_SKILLS_LINK: &str = ".kiro/skills";

const GROK_SKILLS_LINK: &str = ".grok/skills";

fn claude_hooks() -> HookLayout {
    HookLayout {
        file: ".claude/settings.json",
        section: "hooks",
        format: DocumentFormat::Json,
        shape: DocumentShape::Array,
        root_defaults: None,
        events: None,
    }
}

fn claude_hook_root() -> String {
    format!("${{CLAUDE_PROJECT_DIR}}/{SHARED_HOOKS_DIR}")
}

const RELATIVE_HOOK_ROOT: &str = SHARED_HOOKS_DIR;

// A user-level install must name an absolute path: a project-relative one would run whatever
// script the currently open project ships.
fn hook_root(project: &ProjectPaths, project_scoped: &str) -> String {
    if project.scope == Some(InstallScope::User) {
        format!(
            "{}/{SHARED_HOOKS_DIR}",
            crate::util::path::to_slash(&project.root)
        )
    } else {
        project_scoped.to_owned()
    }
}

// Key order is the digest input; reordering renames every installed hook.
fn claude_hook(hook: &MergedHook, root: &str) -> JsonValue {
    let mut command = JsonObject::new();

    command.insert("type".to_owned(), json!("command"));
    command.insert("command".to_owned(), json!(hook_command(hook, root)));

    if let Some(timeout) = hook.timeout {
        command.insert("timeout".to_owned(), json!(timeout));
    }

    let mut entry = JsonObject::new();

    if let Some(matcher) = &hook.matcher {
        entry.insert("matcher".to_owned(), json!(matcher));
    }

    entry.insert(
        "hooks".to_owned(),
        JsonValue::Array(vec![JsonValue::Object(command)]),
    );
    JsonValue::Object(entry)
}

// Not `[hooks]` in `.codex/config.toml`: that is an array-of-tables, which the TOML driver refuses.
fn codex_hooks() -> HookLayout {
    HookLayout {
        file: ".codex/hooks.json",
        section: "hooks",
        format: DocumentFormat::Json,
        shape: DocumentShape::Array,
        root_defaults: None,
        events: None,
    }
}

#[allow(clippy::unnecessary_wraps)]
fn cursor_event(event: HookEvent) -> Option<&'static str> {
    Some(match event {
        HookEvent::SessionStart => "sessionStart",
        HookEvent::UserPromptSubmit => "userPromptSubmit",
        HookEvent::PreToolUse => "preToolUse",
        HookEvent::PostToolUse => "postToolUse",
        HookEvent::Stop => "stop",
        HookEvent::SubagentStop => "subagentStop",
        HookEvent::PreCompact => "preCompact",
        HookEvent::SessionEnd => "sessionEnd",
    })
}

fn cursor_hooks() -> HookLayout {
    let mut root_defaults = JsonObject::new();

    root_defaults.insert("version".to_owned(), json!(1));

    HookLayout {
        file: ".cursor/hooks.json",
        section: "hooks",
        format: DocumentFormat::Json,
        shape: DocumentShape::Array,
        root_defaults: Some(root_defaults),
        events: Some(cursor_event),
    }
}

// Key order is the digest input; reordering renames every installed hook.
fn cursor_hook(hook: &MergedHook, root: &str) -> JsonValue {
    let mut entry = JsonObject::new();

    entry.insert("command".to_owned(), json!(hook_command(hook, root)));

    if let Some(timeout) = hook.timeout {
        entry.insert("timeout".to_owned(), json!(timeout));
    }

    JsonValue::Object(entry)
}

fn url(transport: &HttpTransport, style: EnvRefStyle) -> String {
    translate_refs(&transport.url, style)
}

fn headers_for(transport: &HttpTransport, style: EnvRefStyle) -> Option<JsonObject> {
    let mut declared: IndexMap<String, String> = IndexMap::new();

    if let Some(variable) = &transport.bearer_token_env_var {
        declared.insert(
            "Authorization".to_owned(),
            format!("Bearer ${{{variable}}}"),
        );
    }

    for (name, value) in &transport.headers {
        declared.insert(name.clone(), value.clone());
    }

    if declared.is_empty() {
        return None;
    }

    declared.sort_by(|a, _, b, _| js_cmp(a, b));

    Some(
        declared
            .into_iter()
            .map(|(name, value)| (name, json!(translate_refs(&value, style))))
            .collect(),
    )
}

fn env_object(
    mcp: &MergedMcp,
    transport: &StdioTransport,
    style: EnvRefStyle,
) -> Option<JsonValue> {
    stdio_env(&expected_env(&mcp.expects), &transport.env, style).map(|env| {
        JsonValue::Object(
            env.into_iter()
                .map(|(name, value)| (name, JsonValue::String(value)))
                .collect(),
        )
    })
}

fn stdio(mcp: &MergedMcp, transport: &StdioTransport, style: EnvRefStyle) -> JsonObject {
    let mut server = JsonObject::new();

    server.insert("command".to_owned(), json!(transport.command));

    if !transport.args.is_empty() {
        server.insert(
            "args".to_owned(),
            JsonValue::Array(
                transport
                    .args
                    .iter()
                    .map(|arg| json!(translate_refs(arg, style)))
                    .collect(),
            ),
        );
    }

    if let Some(env) = env_object(mcp, transport, style) {
        server.insert("env".to_owned(), env);
    }

    server
}

fn remote(transport: &HttpTransport, style: EnvRefStyle, kind: Option<&str>) -> JsonValue {
    let mut server = JsonObject::new();

    if let Some(kind) = kind {
        server.insert("type".to_owned(), json!(kind));
    }

    server.insert("url".to_owned(), json!(url(transport, style)));

    if let Some(headers) = headers_for(transport, style) {
        server.insert("headers".to_owned(), JsonValue::Object(headers));
    }

    JsonValue::Object(server)
}

// Claude Code reads a server with no `type` as stdio.
fn claude_server(mcp: &MergedMcp) -> JsonValue {
    match &mcp.transport {
        McpTransport::Stdio(transport) => JsonValue::Object(stdio(mcp, transport, shell_ref)),
        McpTransport::Http(transport) => remote(transport, shell_ref, Some("http")),
    }
}

fn claude_hook_config(hook: &MergedHook, project: &ProjectPaths) -> JsonValue {
    claude_hook(hook, &hook_root(project, &claude_hook_root()))
}

pub static CLAUDE: LazyLock<HarnessProfile> = LazyLock::new(|| HarnessProfile {
    name: "claude",
    skills_link: Some(CLAUDE_SKILLS_LINK),
    mcp: McpLayout {
        file: ".mcp.json",
        user_file: Some(".claude.json"),
        section: "mcpServers",
        format: DocumentFormat::Json,
    },
    server_config: claude_server,
    hooks: Some(claude_hooks()),
    hook_config: Some(claude_hook_config),
});

fn cursor_server(mcp: &MergedMcp) -> JsonValue {
    match &mcp.transport {
        McpTransport::Stdio(transport) => JsonValue::Object(stdio(mcp, transport, shell_ref)),
        McpTransport::Http(transport) => remote(transport, namespaced_ref, None),
    }
}

fn cursor_hook_config(hook: &MergedHook, project: &ProjectPaths) -> JsonValue {
    cursor_hook(hook, &hook_root(project, RELATIVE_HOOK_ROOT))
}

pub static CURSOR: LazyLock<HarnessProfile> = LazyLock::new(|| HarnessProfile {
    name: "cursor",
    skills_link: Some(CLAUDE_SKILLS_LINK),
    mcp: McpLayout {
        file: ".cursor/mcp.json",
        user_file: None,
        section: "mcpServers",
        format: DocumentFormat::Json,
    },
    server_config: cursor_server,
    hooks: Some(cursor_hooks()),
    hook_config: Some(cursor_hook_config),
});

// Not `${input:VAR}`: it only works with a matching `inputs` array, which ambit does not write.
fn copilot_server(mcp: &MergedMcp) -> JsonValue {
    match &mcp.transport {
        McpTransport::Stdio(transport) => {
            let mut server = JsonObject::new();

            server.insert("type".to_owned(), json!("stdio"));
            server.extend(stdio(mcp, transport, namespaced_ref));
            JsonValue::Object(server)
        }
        McpTransport::Http(transport) => remote(transport, namespaced_ref, Some("http")),
    }
}

pub static COPILOT: LazyLock<HarnessProfile> = LazyLock::new(|| HarnessProfile {
    name: "copilot",
    skills_link: None,
    mcp: McpLayout {
        file: ".vscode/mcp.json",
        user_file: None,
        section: "servers",
        format: DocumentFormat::Json,
    },
    server_config: copilot_server,
    hooks: Some(claude_hooks()),
    hook_config: Some(claude_hook_config),
});

fn codex_server(mcp: &MergedMcp) -> JsonValue {
    let transport = match &mcp.transport {
        McpTransport::Stdio(transport) => {
            return JsonValue::Object(stdio(mcp, transport, shell_ref));
        }
        McpTransport::Http(transport) => transport,
    };

    let mut literal = JsonObject::new();
    let mut from_env = JsonObject::new();
    let mut declared: Vec<(&String, &String)> = transport.headers.iter().collect();

    declared.sort_by(|(a, _), (b, _)| js_cmp(a, b));

    for (name, value) in declared {
        match sole_reference(value) {
            None => {
                literal.insert(name.clone(), json!(translate_refs(value, shell_ref)));
            }
            Some(sole) => {
                from_env.insert(name.clone(), json!(sole));
            }
        }
    }

    let mut server = JsonObject::new();

    server.insert("url".to_owned(), json!(url(transport, shell_ref)));

    if let Some(variable) = &transport.bearer_token_env_var {
        server.insert("bearer_token_env_var".to_owned(), json!(variable));
    }

    if !literal.is_empty() {
        server.insert("http_headers".to_owned(), JsonValue::Object(literal));
    }

    if !from_env.is_empty() {
        server.insert("env_http_headers".to_owned(), JsonValue::Object(from_env));
    }

    JsonValue::Object(server)
}

fn codex_hook_config(hook: &MergedHook, project: &ProjectPaths) -> JsonValue {
    claude_hook(hook, &hook_root(project, RELATIVE_HOOK_ROOT))
}

pub static CODEX: LazyLock<HarnessProfile> = LazyLock::new(|| HarnessProfile {
    name: "codex",
    skills_link: None,
    mcp: McpLayout {
        file: ".codex/config.toml",
        user_file: None,
        section: "mcp_servers",
        format: DocumentFormat::Toml,
    },
    server_config: codex_server,
    hooks: Some(codex_hooks()),
    hook_config: Some(codex_hook_config),
});

fn opencode_server(mcp: &MergedMcp) -> JsonValue {
    match &mcp.transport {
        McpTransport::Stdio(transport) => {
            let mut command = vec![json!(transport.command)];

            command.extend(
                transport
                    .args
                    .iter()
                    .map(|arg| json!(translate_refs(arg, shell_ref))),
            );

            let mut server = JsonObject::new();

            server.insert("type".to_owned(), json!("local"));
            server.insert("command".to_owned(), JsonValue::Array(command));

            if let Some(env) = env_object(mcp, transport, shell_ref) {
                server.insert("environment".to_owned(), env);
            }

            JsonValue::Object(server)
        }
        McpTransport::Http(transport) => remote(transport, braced_ref, Some("remote")),
    }
}

pub static OPENCODE: LazyLock<HarnessProfile> = LazyLock::new(|| HarnessProfile {
    name: "opencode",
    skills_link: None,
    mcp: McpLayout {
        file: ".opencode/opencode.jsonc",
        user_file: None,
        section: "mcp",
        format: DocumentFormat::Jsonc,
    },
    server_config: opencode_server,
    hooks: None,
    hook_config: None,
});

fn gemini_event(event: HookEvent) -> Option<&'static str> {
    match event {
        HookEvent::SessionStart => Some("SessionStart"),
        HookEvent::UserPromptSubmit => Some("BeforeAgent"),
        HookEvent::PreToolUse => Some("BeforeTool"),
        HookEvent::PostToolUse => Some("AfterTool"),
        HookEvent::Stop => Some("AfterAgent"),
        // Not `AfterAgent`, which fires for the main agent's turn.
        HookEvent::SubagentStop => None,
        HookEvent::PreCompact => Some("PreCompress"),
        HookEvent::SessionEnd => Some("SessionEnd"),
    }
}

fn gemini_tool(name: &str) -> Option<&'static str> {
    match name {
        "Bash" => Some("run_shell_command"),
        "Read" => Some("read_file"),
        "Write" => Some("write_file"),
        "Edit" => Some("replace"),
        "Glob" => Some("glob"),
        "Grep" => Some("grep_search"),
        "LS" => Some("list_directory"),
        "WebFetch" => Some("web_fetch"),
        "WebSearch" => Some("google_web_search"),
        "TodoWrite" => Some("write_todos"),
        _ => None,
    }
}

fn gemini_matcher(matcher: &str) -> String {
    matcher
        .split('|')
        .map(|token| gemini_tool(token).unwrap_or(token))
        .collect::<Vec<_>>()
        .join("|")
}

fn gemini_hooks() -> HookLayout {
    HookLayout {
        file: ".gemini/settings.json",
        section: "hooks",
        format: DocumentFormat::Json,
        shape: DocumentShape::Array,
        root_defaults: None,
        events: Some(gemini_event),
    }
}

fn gemini_hook_root() -> String {
    format!("$GEMINI_PROJECT_DIR/{SHARED_HOOKS_DIR}")
}

// Key order is the digest input; reordering renames every installed hook.
fn gemini_hook(hook: &MergedHook, root: &str) -> JsonValue {
    let mut command = JsonObject::new();

    command.insert("name".to_owned(), json!(hook.name));
    command.insert("type".to_owned(), json!("command"));
    command.insert("command".to_owned(), json!(hook_command(hook, root)));

    if let Some(timeout) = hook.timeout {
        command.insert("timeout".to_owned(), json!(timeout.saturating_mul(1000)));
    }

    let mut entry = JsonObject::new();

    if let Some(matcher) = &hook.matcher {
        entry.insert("matcher".to_owned(), json!(gemini_matcher(matcher)));
    }

    entry.insert(
        "hooks".to_owned(),
        JsonValue::Array(vec![JsonValue::Object(command)]),
    );
    JsonValue::Object(entry)
}

fn gemini_hook_config(hook: &MergedHook, project: &ProjectPaths) -> JsonValue {
    gemini_hook(hook, &hook_root(project, &gemini_hook_root()))
}

fn gemini_server(mcp: &MergedMcp) -> JsonValue {
    let transport = match &mcp.transport {
        McpTransport::Stdio(transport) => {
            return JsonValue::Object(stdio(mcp, transport, shell_ref));
        }
        McpTransport::Http(transport) => transport,
    };

    let mut server = JsonObject::new();

    // Gemini reads `url` as SSE; `httpUrl` is streamable HTTP.
    server.insert("httpUrl".to_owned(), json!(url(transport, shell_ref)));

    if let Some(headers) = headers_for(transport, shell_ref) {
        server.insert("headers".to_owned(), JsonValue::Object(headers));
    }

    JsonValue::Object(server)
}

pub static GEMINI: LazyLock<HarnessProfile> = LazyLock::new(|| HarnessProfile {
    name: "gemini",
    skills_link: None,
    mcp: McpLayout {
        file: ".gemini/settings.json",
        user_file: None,
        section: "mcpServers",
        format: DocumentFormat::Json,
    },
    server_config: gemini_server,
    hooks: Some(gemini_hooks()),
    hook_config: Some(gemini_hook_config),
});

fn kiro_event(event: HookEvent) -> Option<&'static str> {
    match event {
        HookEvent::SessionStart => Some("SessionStart"),
        HookEvent::UserPromptSubmit => Some("UserPromptSubmit"),
        HookEvent::PreToolUse => Some("PreToolUse"),
        HookEvent::PostToolUse => Some("PostToolUse"),
        HookEvent::Stop => Some("Stop"),
        HookEvent::SubagentStop | HookEvent::PreCompact => None,
        HookEvent::SessionEnd => Some("SessionEnd"),
    }
}

fn kiro_hooks() -> HookLayout {
    let mut root_defaults = JsonObject::new();

    root_defaults.insert("version".to_owned(), json!("v1"));

    HookLayout {
        file: ".kiro/hooks/ambit.json",
        section: "hooks",
        format: DocumentFormat::Json,
        shape: DocumentShape::List,
        root_defaults: Some(root_defaults),
        events: Some(kiro_event),
    }
}

// Key order is the digest input; reordering renames every installed hook.
fn kiro_hook(hook: &MergedHook, root: &str) -> JsonValue {
    let mut action = JsonObject::new();

    action.insert("type".to_owned(), json!("command"));
    action.insert("command".to_owned(), json!(hook_command(hook, root)));

    let mut entry = JsonObject::new();

    entry.insert("name".to_owned(), json!(hook.name));
    entry.insert(
        LIST_EVENT_FIELD.to_owned(),
        json!(kiro_event(hook.event).unwrap_or(hook.event.as_str())),
    );

    if let Some(matcher) = &hook.matcher {
        entry.insert("matcher".to_owned(), json!(matcher));
    }

    entry.insert("action".to_owned(), JsonValue::Object(action));

    if let Some(timeout) = hook.timeout {
        entry.insert("timeout".to_owned(), json!(timeout));
    }

    JsonValue::Object(entry)
}

fn kiro_hook_config(hook: &MergedHook, project: &ProjectPaths) -> JsonValue {
    kiro_hook(hook, &hook_root(project, RELATIVE_HOOK_ROOT))
}

fn kiro_server(mcp: &MergedMcp) -> JsonValue {
    match &mcp.transport {
        McpTransport::Stdio(transport) => JsonValue::Object(stdio(mcp, transport, shell_ref)),
        McpTransport::Http(transport) => remote(transport, shell_ref, None),
    }
}

pub static KIRO: LazyLock<HarnessProfile> = LazyLock::new(|| HarnessProfile {
    name: "kiro",
    skills_link: Some(KIRO_SKILLS_LINK),
    mcp: McpLayout {
        file: ".kiro/settings/mcp.json",
        user_file: None,
        section: "mcpServers",
        format: DocumentFormat::Json,
    },
    server_config: kiro_server,
    hooks: Some(kiro_hooks()),
    hook_config: Some(kiro_hook_config),
});

fn grok_server(mcp: &MergedMcp) -> JsonValue {
    match &mcp.transport {
        McpTransport::Stdio(transport) => JsonValue::Object(stdio(mcp, transport, shell_ref)),
        McpTransport::Http(transport) => remote(transport, shell_ref, None),
    }
}

pub static GROK: LazyLock<HarnessProfile> = LazyLock::new(|| HarnessProfile {
    name: "grok",
    skills_link: Some(GROK_SKILLS_LINK),
    mcp: McpLayout {
        file: ".grok/config.toml",
        user_file: None,
        section: "mcp_servers",
        format: DocumentFormat::Toml,
    },
    server_config: grok_server,
    hooks: Some(claude_hooks()),
    hook_config: Some(claude_hook_config),
});

fn devin_server(mcp: &MergedMcp) -> JsonValue {
    let transport = match &mcp.transport {
        McpTransport::Stdio(transport) => {
            return JsonValue::Object(stdio(mcp, transport, namespaced_ref));
        }
        McpTransport::Http(transport) => transport,
    };

    let mut server = JsonObject::new();

    server.insert("url".to_owned(), json!(url(transport, namespaced_ref)));
    server.insert("transport".to_owned(), json!("http"));

    if let Some(headers) = headers_for(transport, namespaced_ref) {
        server.insert("headers".to_owned(), JsonValue::Object(headers));
    }

    JsonValue::Object(server)
}

pub static DEVIN: LazyLock<HarnessProfile> = LazyLock::new(|| HarnessProfile {
    name: "devin",
    skills_link: None,
    mcp: McpLayout {
        file: ".devin/mcp_config.json",
        user_file: None,
        section: "mcpServers",
        format: DocumentFormat::Json,
    },
    server_config: devin_server,
    hooks: Some(claude_hooks()),
    hook_config: Some(claude_hook_config),
});

pub static PROFILES: LazyLock<Vec<&'static HarnessProfile>> = LazyLock::new(|| {
    vec![
        &*CLAUDE, &*CODEX, &*COPILOT, &*CURSOR, &*DEVIN, &*GEMINI, &*GROK, &*KIRO, &*OPENCODE,
    ]
});

#[cfg(test)]
mod tests;
