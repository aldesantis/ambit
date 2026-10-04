//! The `exec` digest: what a hook or an MCP server runs, as one value `ambit.lock` records.
//!
//! A hook fires on harness events and a stdio MCP server is spawned with the user's environment, so
//! both are code that runs on the machine. `ambit.lock` records an `exec` digest for every hook and
//! every MCP server (see [`hook_exec`] and [`mcp_exec`]), so a change to what runs is a change to
//! the lock. Every catalog's items get one, `path:` ones included, so the lock has one shape.

use indexmap::IndexMap;

use crate::model::catalog::MergedHook;
use crate::model::hook_entity::HookType;
use crate::model::mcp_entity::McpTransport;
use crate::util::cmp::js_cmp;
use crate::util::hash::fields_digest;

/// The fields one `exec` digest is made of, fed to [`fields_digest`] in order.
#[derive(Default)]
struct ExecFields(Vec<Vec<u8>>);

impl ExecFields {
    fn text(&mut self, text: &str) -> &mut Self {
        self.0.push(text.as_bytes().to_vec());
        self
    }

    /// An optional value as a presence marker, then the value when present, so an absent value and
    /// an empty one differ.
    fn optional(&mut self, text: Option<&str>) -> &mut Self {
        match text {
            Some(text) => self.text("1").text(text),
            None => self.text("0"),
        }
    }

    /// A map as its length, then each key and value, keys in [`js_cmp`] order.
    ///
    /// Sorted because the order a catalog wrote its keys in changes nothing about what runs.
    fn map(&mut self, map: &IndexMap<String, String>) -> &mut Self {
        let mut entries: Vec<(&String, &String)> = map.iter().collect();

        entries.sort_by(|a, b| js_cmp(a.0, b.0));
        self.text(&entries.len().to_string());

        for (key, value) in entries {
            self.text(key).text(value);
        }

        self
    }

    fn digest(&self) -> String {
        fields_digest(self.0.iter().map(Vec::as_slice))
    }
}

/// The `exec` digest of a hook: what decides when it fires and what it runs.
///
/// The fields are `hook`, its type, its event, its matcher (optional), and its command as written.
/// A script hook adds its tree digest (optional), so a changed script is changed execution even
/// when the command line is not. `tree` is that digest, which [`item_digests`] computes only for a
/// source with a commit: a `path:` script hook's digest covers its command line alone, for the
/// reason `ambit.lock` records no tree digest for it (see `project/lock.rs`).
///
/// The timeout and description are left out: neither changes what runs.
///
/// [`item_digests`]: crate::project::lock::item_digests
pub fn hook_exec(hook: &MergedHook, tree: Option<&str>) -> String {
    let mut fields = ExecFields::default();

    fields
        .text("hook")
        .text(hook.r#type.as_str())
        .text(hook.event.as_str())
        .optional(hook.matcher.as_deref())
        .text(&hook.command);

    if hook.r#type == HookType::Script {
        fields.optional(tree);
    }

    fields.digest()
}

/// The `exec` digest of an MCP server's transport.
///
/// For stdio: `mcp`, `stdio`, the command, the argument count, each argument in order, and the
/// environment as a map. The environment is included because a variable can change what a process
/// runs as surely as an argument can (`NODE_OPTIONS`, `PYTHONPATH`). For http: `mcp`, `http`, the
/// URL, the bearer token variable (optional), and the headers as a map. Values are the catalog's
/// own text, `${VAR}` references left unresolved, so no secret reaches the digest, and a changed
/// value can mean a different secret is sent.
pub fn mcp_exec(transport: &McpTransport) -> String {
    let mut fields = ExecFields::default();

    fields.text("mcp").text(transport.kind().as_str());

    match transport {
        McpTransport::Stdio(stdio) => {
            fields
                .text(&stdio.command)
                .text(&stdio.args.len().to_string());

            for arg in &stdio.args {
                fields.text(arg);
            }

            fields.map(&stdio.env);
        }
        McpTransport::Http(http) => {
            fields
                .text(&http.url)
                .optional(http.bearer_token_env_var.as_deref())
                .map(&http.headers);
        }
    }

    fields.digest()
}

#[cfg(test)]
mod tests;
