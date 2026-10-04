//! The execution gate: what an install refuses to introduce until someone accepts it.
//!
//! A hook fires on harness events and a stdio MCP server is spawned with the user's environment, so
//! both are code that runs on the machine. `ambit.lock` records an `exec` digest for every hook and
//! every MCP server (see [`hook_exec`] and [`mcp_exec`]), and an install compares the digests it is
//! about to write with the ones the project's lock holds. A hook or stdio server from a catalog of
//! [`Trust::Review`] that is new, or whose digest moved, refuses the install with exit 5 until it
//! is re-run with `--accept-exec`, which writes the new digests into the lock. An http server runs
//! nothing locally, so a new or changed one is only a warning.
//!
//! Every catalog's items get a digest, `trust: full` and `path:` ones included, so the lock has one
//! shape and a catalog switched to `review` is compared from its first install on.
//!
//! A project with no lock has nothing to compare against, so everything executable from a review
//! catalog is new. Deleting the lock is therefore not a way around the gate. `--frozen` skips the
//! gate: a frozen install has already proved the lock would not change, so nothing is new.
//!
//! A lock written before `exec` was recorded has entries without one. Such an entry counts as
//! accepted only when its catalog's recorded commit is the one being installed: the same commit
//! carries the same definitions, so the item is what the project already ran. Any other entry
//! without a digest counts as changed. Upgrading ambit thus gates nothing on a plain reinstall,
//! and is not a way to slip a moved catalog past the gate either.

use indexmap::IndexMap;

use crate::errors::{Result, drift_error};
use crate::model::catalog::{MergedHook, MergedMcp};
use crate::model::config::Trust;
use crate::model::hook_entity::{HookType, command_program, script_reference};
use crate::model::lock_file::{LockedItem, LockedItems};
use crate::model::mcp_entity::{McpTransport, McpTransportKind};
use crate::model::requirement::ItemKind;
use crate::project::lock::Lock;
use crate::resolution::resolve::Bundle;
use crate::util::cmp::js_cmp;
use crate::util::hash::fields_digest;
use crate::util::text::js_trim;

/// The length of a commit as a refusal prints it.
const SHORT_COMMIT: usize = 7;

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

/// Why an item's execution is not in the lock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecStatus {
    /// The lock has no entry of this name, or there is no lock.
    New,
    /// The entry's `exec` differs, or is missing and not accepted (see the module header).
    Changed,
}

/// One hook or MCP server whose execution the lock does not hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecChange {
    /// [`ItemKind::Hook`] or [`ItemKind::Mcp`].
    pub kind: ItemKind,
    pub name: String,
    pub catalog: String,
    /// The commit its catalog is being installed at, when the source has one.
    pub commit: Option<String>,
    /// When it runs: a hook's event and matcher, or a server's transport.
    pub when: String,
    /// What it runs, as a person reads it: a command line, or an http server's URL.
    pub runs: String,
    pub status: ExecStatus,
}

impl ExecChange {
    /// The parenthesized note a report prints after [`ExecChange::runs`], without the parentheses.
    pub fn note(&self) -> String {
        match self.status {
            ExecStatus::New => match &self.commit {
                Some(commit) => format!(
                    "new, from {}@{}",
                    self.catalog,
                    &commit[..commit.len().min(SHORT_COMMIT)]
                ),
                None => format!("new, from {}", self.catalog),
            },
            ExecStatus::Changed if self.when == McpTransportKind::Http.as_str() => {
                "endpoint changed".to_owned()
            }
            ExecStatus::Changed => "command changed".to_owned(),
        }
    }
}

/// What [`review_exec`] found, split by what an install does about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecReview {
    /// Hooks and stdio servers that refuse the install, hooks first, each in bundle order.
    pub gated: Vec<ExecChange>,
    /// Http servers, which only warn, in bundle order.
    pub endpoints: Vec<ExecChange>,
}

/// Whether `earlier`, an entry with no `exec`, counts as accepted. See the module header.
fn accepted_without_digest(
    previous: &LockedItems,
    earlier: &LockedItem,
    catalog: &str,
    lock: &Lock,
) -> bool {
    let current = lock
        .catalogs
        .get(catalog)
        .and_then(|entry| entry.commit.as_ref());

    earlier.catalog.as_deref() == Some(catalog)
        && current.is_some()
        && previous.catalog_commits.get(catalog) == current
}

/// How one item's `exec` compares with the earlier lock, or `None` when the lock already holds it.
fn status_of(
    previous: Option<&LockedItems>,
    section: impl Fn(&LockedItems) -> &IndexMap<String, LockedItem>,
    name: &str,
    catalog: &str,
    exec: &str,
    lock: &Lock,
) -> Option<ExecStatus> {
    let Some(previous) = previous else {
        return Some(ExecStatus::New);
    };

    let Some(earlier) = section(previous).get(name) else {
        return Some(ExecStatus::New);
    };

    let accepted = match &earlier.exec {
        Some(recorded) => recorded == exec,
        None => accepted_without_digest(previous, earlier, catalog, lock),
    };

    (!accepted).then_some(ExecStatus::Changed)
}

/// Joins words into a command line, double-quoting any that is empty or holds whitespace.
fn command_line<'a>(words: impl IntoIterator<Item = &'a str>) -> String {
    words
        .into_iter()
        .map(|word| {
            if word.is_empty() || word.chars().any(char::is_whitespace) {
                format!("{word:?}")
            } else {
                word.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What a hook runs. A script is shown by its path within the catalog, followed by its arguments.
fn hook_runs(hook: &MergedHook) -> String {
    let command = js_trim(&hook.command);

    if hook.r#type == HookType::Command {
        return command.to_owned();
    }

    let program = command_program(command);
    let arguments = &command[program.len()..];

    format!("{}/{}{arguments}", hook.path, script_reference(&program))
}

/// When a hook fires: its event, then its matcher if it has one.
fn hook_when(hook: &MergedHook) -> String {
    match &hook.matcher {
        Some(matcher) => format!("{} {matcher}", hook.event),
        None => hook.event.to_string(),
    }
}

/// What a server runs: a stdio command line led by its environment, or an http URL.
fn mcp_runs(mcp: &MergedMcp) -> String {
    match &mcp.transport {
        McpTransport::Stdio(stdio) => {
            let mut env: Vec<String> = stdio
                .env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect();

            env.sort_by(|a, b| js_cmp(a, b));

            command_line(
                env.iter()
                    .map(String::as_str)
                    .chain([stdio.command.as_str()])
                    .chain(stdio.args.iter().map(String::as_str)),
            )
        }
        McpTransport::Http(http) => http.url.clone(),
    }
}

/// Every hook and MCP server from a [`Trust::Review`] catalog whose execution `previous` does not
/// hold.
///
/// `previous` is the project's lock as [`read_locked_items`] reads it, `None` when it has none.
/// `lock` is the lock this install would write, and supplies each item's `exec` and its catalog's
/// commit. `trust` is keyed by catalog name; a catalog it does not hold is reviewed.
///
/// Pure, so the caller decides whether the gate applies (`--frozen`, `--accept-exec`).
///
/// [`read_locked_items`]: crate::model::lock_file::read_locked_items
pub fn review_exec(
    previous: Option<&LockedItems>,
    lock: &Lock,
    bundle: &Bundle,
    trust: &IndexMap<String, Trust>,
) -> ExecReview {
    let reviewed =
        |catalog: &str| trust.get(catalog).copied().unwrap_or(Trust::Review) == Trust::Review;
    let commit_of = |catalog: &str| {
        lock.catalogs
            .get(catalog)
            .and_then(|entry| entry.commit.clone())
    };
    let mut review = ExecReview::default();

    for hook in bundle.hooks.iter().filter(|hook| reviewed(&hook.catalog)) {
        let Some(locked) = lock.hooks.get(&hook.name) else {
            continue;
        };

        let Some(status) = status_of(
            previous,
            |items| &items.hooks,
            &hook.name,
            &hook.catalog,
            &locked.exec,
            lock,
        ) else {
            continue;
        };

        review.gated.push(ExecChange {
            kind: ItemKind::Hook,
            name: hook.name.clone(),
            catalog: hook.catalog.clone(),
            commit: commit_of(&hook.catalog),
            when: hook_when(hook),
            runs: hook_runs(hook),
            status,
        });
    }

    for mcp in bundle.mcps.iter().filter(|mcp| reviewed(&mcp.catalog)) {
        let Some(locked) = lock.mcps.get(&mcp.name) else {
            continue;
        };

        let Some(status) = status_of(
            previous,
            |items| &items.mcps,
            &mcp.name,
            &mcp.catalog,
            &locked.exec,
            lock,
        ) else {
            continue;
        };

        let change = ExecChange {
            kind: ItemKind::Mcp,
            name: mcp.name.clone(),
            catalog: mcp.catalog.clone(),
            commit: commit_of(&mcp.catalog),
            when: mcp.transport.kind().to_string(),
            runs: mcp_runs(mcp),
            status,
        };

        match mcp.transport {
            McpTransport::Stdio(_) => review.gated.push(change),
            McpTransport::Http(_) => review.endpoints.push(change),
        }
    }

    review
}

/// The refusal for an install that would add execution the lock does not hold, or `Ok` when
/// `gated` is empty.
///
/// Each item is two detail lines: its kind, name and when it runs, then what it runs with
/// [`ExecChange::note`] beside it, the notes aligned in one column.
///
/// # Errors
///
/// Exit 5 when `gated` holds anything.
pub fn refuse_unaccepted(gated: &[ExecChange]) -> Result<()> {
    if gated.is_empty() {
        return Ok(());
    }

    let width = gated
        .iter()
        .map(|change| change.runs.chars().count())
        .max()
        .unwrap_or_default();
    let mut detail = Vec::new();

    for change in gated {
        let pad = width - change.runs.chars().count();

        detail.push(format!("{} {}  {}", change.kind, change.name, change.when));
        detail.push(format!(
            "  {}{}   ({})",
            change.runs,
            " ".repeat(pad),
            change.note()
        ));
    }

    detail.push("review the change, then re-run with `--accept-exec`".to_owned());

    Err(drift_error(
        "install would add execution that was not in the lock",
        detail,
    ))
}

#[cfg(test)]
mod tests;
