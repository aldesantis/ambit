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

const SHORT_COMMIT: usize = 7;

#[derive(Default)]
struct ExecFields(Vec<Vec<u8>>);

impl ExecFields {
    fn text(&mut self, text: &str) -> &mut Self {
        self.0.push(text.as_bytes().to_vec());
        self
    }

    fn optional(&mut self, text: Option<&str>) -> &mut Self {
        match text {
            Some(text) => self.text("1").text(text),
            None => self.text("0"),
        }
    }

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecStatus {
    New,
    Changed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecChange {
    pub kind: ItemKind,
    pub name: String,
    pub catalog: String,
    pub commit: Option<String>,
    pub when: String,
    pub runs: String,
    pub status: ExecStatus,
}

impl ExecChange {
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecReview {
    pub gated: Vec<ExecChange>,
    pub endpoints: Vec<ExecChange>,
}

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

fn hook_runs(hook: &MergedHook) -> String {
    let command = js_trim(&hook.command);

    if hook.r#type == HookType::Command {
        return command.to_owned();
    }

    let program = command_program(command);
    let arguments = &command[program.len()..];

    format!("{}/{}{arguments}", hook.path, script_reference(&program))
}

fn hook_when(hook: &MergedHook) -> String {
    match &hook.matcher {
        Some(matcher) => format!("{} {matcher}", hook.event),
        None => hook.event.to_string(),
    }
}

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
