//! Profiles for the five supported harnesses.
//!
//! Each profile's server shape matches what that tool's own documentation tells a person to write
//! by hand, so an ambit-generated config is indistinguishable from a hand-written one.
//!
//! Skills: Claude Code and Cursor read `.claude/skills`, so both get a link to the shared
//! directory. Codex, Copilot and opencode read `.agents/skills` natively and need no link.
//!
//! Hooks: Claude and Copilot share both the file (`.claude/settings.json`) and its renderer. Codex
//! shares the renderer but not the file (its entries live in `.codex/hooks.json`). Cursor shares
//! neither: its own file, its own event names, its own entry shape. opencode has no declarative
//! hooks; a hook selected for it is reported as skipped (`skipped_hooks`, `profile.rs`).
//!
//! How a hook's script is addressed is the one thing no profile decides on its own: every harness
//! reads the file it is handed as user config when it sits under the home directory, and a
//! user-level file has no project to be relative to. See [`hook_root`].

use std::sync::LazyLock;

use indexmap::IndexMap;
use serde_json::json;

use crate::harness::adapter::{InstallScope, ProjectPaths};
use crate::harness::env::{
    EnvRefStyle, braced_ref, namespaced_ref, shell_ref, sole_reference, stdio_env, translate_refs,
};
use crate::harness::profile::{HarnessProfile, HookLayout, McpLayout, SHARED_HOOKS_DIR};
use crate::model::catalog::{MergedHook, MergedMcp, hook_command};
use crate::model::documents::{DocumentFormat, DocumentShape};
use crate::model::expectation::expected_env;
use crate::model::hook_entity::HookEvent;
use crate::model::mcp_entity::{HttpTransport, McpTransport, StdioTransport};
use crate::util::cmp::js_cmp;
use crate::util::json::{JsonObject, JsonValue};

/// Where Claude Code and Cursor look for skills.
const CLAUDE_SKILLS_LINK: &str = ".claude/skills";

/// Claude Code's hooks file. Also read natively by Copilot.
///
/// The section is `Array`-shaped, not `Map`-shaped, because the file is the user's own: their
/// `model`, `permissions`, and hand-written hooks live in it. ambit owns entries inside
/// `hooks.<Event>` by digest, not the `hooks` root.
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

/// Where Claude Code resolves a materialized hook script from, in a project install.
///
/// `${CLAUDE_PROJECT_DIR}` is Claude's own documented placeholder for referencing hook scripts
/// relative to the project root, regardless of the session's working directory. A relative path
/// cannot promise that.
///
/// Also written for Copilot, which reads this same file. Whether Copilot interpolates this
/// placeholder is undocumented either way; see [`COPILOT`].
fn claude_hook_root() -> String {
    format!("${{CLAUDE_PROJECT_DIR}}/{SHARED_HOOKS_DIR}")
}

/// Where harnesses with no placeholder resolve a hook script from in a project install: the path
/// as written, project-relative.
///
/// Cursor and Codex interpolate nothing in a `command`. Cursor documents project hooks as running
/// from the project root, with `./hooks/script.sh` resolving to `<project>/hooks/script.sh`;
/// nothing confines that to `.cursor/`, so a script under `.agents/` resolves the same way.
///
/// Not Codex's own documented suggestion of `$(git rev-parse --show-toplevel)/…`: that requires git
/// and a POSIX shell. A relative path still misses if a session's cwd is not the project root, but
/// that is the harness's own limitation, the same one a person writing the hook by hand would hit.
const RELATIVE_HOOK_ROOT: &str = SHARED_HOOKS_DIR;

/// Where a hook script is resolved from: `project_scoped` for a project install, the expanded
/// install root for a user-level one.
///
/// A user-level file is read in every project on the machine, so nothing project-relative can
/// reach the scripts ambit installed. `${CLAUDE_PROJECT_DIR}` and a bare relative path both resolve
/// inside whatever project is open: in one that has no such file the hook silently never runs, and
/// in one that happens to ship `.agents/hooks/<name>/<name>.sh` the harness runs *that* project's
/// script with the user's settings behind it, which would make cloning a repository enough to get
/// code run.
///
/// Expanded rather than `$HOME/…`: Cursor and Codex interpolate nothing in a `command`, and ambit
/// does not assume a POSIX shell expands one (see [`RELATIVE_HOOK_ROOT`]). The path is therefore
/// machine-specific, which is what a user-level config file is anyway.
///
/// `/` rather than a path join, since this is joined to a `/`-separated artifact path either way,
/// and every shell and harness accepts a forward slash on the platforms ambit runs on.
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

/// One hook, Claude-shaped: an entry in `hooks.<Event>` pairing an optional tool `matcher` with
/// the commands to run.
///
/// One entry carries one command, because one declaration is one hook; grouping several under one
/// entry would make a digest name a set whose membership changes as other hooks come and go.
///
/// Copilot reads exactly this shape and ignores `matcher`. Codex reads it too, differing only in
/// `root`, which is why `root` is a parameter here rather than a constant.
///
/// Key order here is the digest's input, so it is fixed in this one place only.
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

/// Where Codex keeps its hooks: `.codex/hooks.json`, holding Claude's own entry shape.
///
/// A separate file rather than `[hooks]` in `.codex/config.toml`, which Codex also reads: a TOML
/// `hooks` table is an array-of-tables (`[[hooks.PreToolUse]]`), and the TOML driver refuses that
/// shape. Reaching for `config.toml` would mean a second driver for no benefit, since Codex reads
/// JSON just as well. No `root_defaults` either, since this file holds hooks and nothing else.
///
/// Codex's hooks are experimental, gated behind `[features] codex_hooks = true` in the user's own
/// config, which ambit does not write into. `doctor` reports this.
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

/// How Cursor spells each of ambit's events: the same names, camelCased.
///
/// The only harness that needs a map; Claude, Copilot and Codex read the `PascalCase` spellings
/// verbatim. Written out rather than derived, because the mapping is a fact about Cursor: the
/// `match` is total over [`HookEvent`], so adding an event without a spelling here is a compile
/// error.
fn cursor_event(event: HookEvent) -> &'static str {
    match event {
        HookEvent::SessionStart => "sessionStart",
        HookEvent::UserPromptSubmit => "userPromptSubmit",
        HookEvent::PreToolUse => "preToolUse",
        HookEvent::PostToolUse => "postToolUse",
        HookEvent::Stop => "stop",
        HookEvent::SubagentStop => "subagentStop",
        HookEvent::PreCompact => "preCompact",
        HookEvent::SessionEnd => "sessionEnd",
    }
}

/// Where Cursor keeps its hooks: `.cursor/hooks.json`, with a `version` beside them.
///
/// `version` is a `root_defaults` value rather than something the renderer writes, because it
/// belongs to the document, not to an entry: ambit seeds it at 1 when creating the file, and leaves
/// an existing `version: 2` in place unless ambit replaces the whole document.
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

/// One hook, Cursor-shaped: a flat entry naming the command, in the array for its camelCased
/// event.
///
/// Cursor nests nothing (one entry is one command, so there is no inner `hooks` array) and has no
/// field for a tool `matcher`, so a matcher is dropped. Declaring a matcher on an unmatchable event
/// is therefore a parse-time error: a dropped matcher would otherwise install the hook unfiltered
/// rather than not at all, which is the surprising outcome.
///
/// A shipped script is named project-relative, because Cursor interpolates nothing in a `command`;
/// see [`RELATIVE_HOOK_ROOT`]. `root` is a parameter for the same reason it is one on
/// [`claude_hook`]: a user-level install cannot name a path relative to a project.
///
/// Key order here is the digest's input, so it is fixed in this one place only.
fn cursor_hook(hook: &MergedHook, root: &str) -> JsonValue {
    let mut entry = JsonObject::new();

    entry.insert("command".to_owned(), json!(hook_command(hook, root)));

    if let Some(timeout) = hook.timeout {
        entry.insert("timeout".to_owned(), json!(timeout));
    }

    JsonValue::Object(entry)
}

/// The remote half of a server: its url, with references translated.
///
/// Every string reaching a config file from the catalog is translated into the syntax the reading
/// harness expands (e.g. a tenant endpoint like `https://${TENANT}.example.com/mcp`), so it is not
/// left as a literal `${TENANT}` in the file.
fn url(transport: &HttpTransport, style: EnvRefStyle) -> String {
    translate_refs(&transport.url, style)
}

/// Headers with `${VAR}` rewritten into one harness's syntax, sorted by name.
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

/// The env map [`stdio_env`] builds, as a JSON object.
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

/// The stdio half of a server, shared by every harness that spells it `command`/`args`/`env`.
///
/// `args` also carries `${VAR}` references translated, since a server invoked through a bridge
/// like `mcp-remote` can take its credential as an argument.
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

/// An `http` server as `type`/`url`/`headers`, with `type` written only when `kind` is given.
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

fn claude_server(mcp: &MergedMcp) -> JsonValue {
    match &mcp.transport {
        McpTransport::Stdio(transport) => JsonValue::Object(stdio(mcp, transport, shell_ref)),
        McpTransport::Http(transport) => remote(transport, shell_ref, Some("http")),
    }
}

fn claude_hook_config(hook: &MergedHook, project: &ProjectPaths) -> JsonValue {
    claude_hook(hook, &hook_root(project, &claude_hook_root()))
}

/// Claude Code.
///
/// `type` is emitted for `http` because the harness treats a server without one as stdio, and
/// omitted for stdio itself, where `command` already says so.
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

/// Cursor. Infers the transport from the presence of `url`, so it wants no `type`.
///
/// Its hooks live in their own file, under their own event names, in an entry shape unlike
/// Claude's. All three differences are captured here: a layout, a map, and a renderer.
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

/// GitHub Copilot in VS Code. Its section is `servers`, and it wants an explicit `type` on both
/// transports.
///
/// Uses `${env:VAR}` throughout, including in a stdio server's `env`. VS Code also supports
/// `${input:VAR}`, which prompts the user, but only when the file declares a matching entry in its
/// own `inputs` array; ambit does not write one, so this form is not used.
///
/// Its hooks are Claude's outright: Copilot reads `.claude/settings.json` natively, so this
/// profile reuses Claude's layout and renderer, including the `${CLAUDE_PROJECT_DIR}` placeholder.
/// That placeholder is undocumented for Copilot specifically: VS Code documents parsing Claude's
/// format and expanding `${CLAUDE_PLUGIN_ROOT}` for Claude-format plugins, but no project-root
/// token, so this may resolve to a literal string rather than a path.
///
/// Written anyway: a separate spelling would require two entries in one array for one declared
/// hook, and both Copilot and Claude would run it, so every project would see the hook twice.
/// `doctor` is where a harness limitation like this gets surfaced if it turns out to matter.
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

/// Codex. TOML, and the one harness with a first-class way to keep a credential out of the file.
///
/// The catalog's bearer-token variable becomes `bearer_token_env_var`. A header whose value is
/// nothing but a `${VAR}` reference becomes `env_http_headers`; other values remain static.
///
/// Its hooks live in `.codex/hooks.json` using Claude's own entry shape, so this profile reuses
/// Claude's renderer; the file itself is Codex's own.
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

/// opencode. JSONC, `mcp` as its section, and its own vocabulary: `local`/`remote` rather than
/// stdio/http, one `command` array rather than a command and its arguments, and `environment` for
/// the env map.
///
/// Has no declarative hooks; it runs TypeScript plugins instead, which ambit cannot generate from a
/// declaration. No `hooks` field is set, and a project that selects a hook for opencode is told the
/// hook was skipped.
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

/// Every profile this build ships, in the order `--help` and error messages list them.
pub static PROFILES: LazyLock<Vec<&'static HarnessProfile>> =
    LazyLock::new(|| vec![&*CLAUDE, &*CODEX, &*COPILOT, &*CURSOR, &*OPENCODE]);

#[cfg(test)]
mod tests;
