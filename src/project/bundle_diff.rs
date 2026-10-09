use std::path::{Path, PathBuf};

use indexmap::IndexSet;
use serde_json::json;

use crate::errors::Result;
use crate::harness::profile::SHARED_HOOKS_DIR;
use crate::model::catalog::{MergedHook, MergedMcp, MergedPack, MergedSkill, hook_command};
use crate::model::mcp_entity::McpTransport;
use crate::resolution::resolve::{Bundle, BundleItem, ItemKind, format_reason, reason_of};
use crate::util::cmp::js_cmp;
use crate::util::fs::{EntryKind, lstat_kind, read_dir_names};
use crate::util::json::{JsonObject, JsonValue, stringify};
use crate::util::string_enum;

string_enum! {
    pub enum BundleChangeKind {
        Added => "added",
        Changed => "changed",
        Removed => "removed",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleChange {
    pub kind: ItemKind,
    pub name: String,
    pub change: BundleChangeKind,
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleDiff {
    pub packs: Vec<BundleChange>,
    pub skills: Vec<BundleChange>,
    pub mcps: Vec<BundleChange>,
    pub hooks: Vec<BundleChange>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BundleChangeCounts {
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
}

pub fn all_changes(diff: &BundleDiff) -> Vec<BundleChange> {
    diff.packs
        .iter()
        .chain(&diff.skills)
        .chain(&diff.mcps)
        .chain(&diff.hooks)
        .cloned()
        .collect()
}

pub fn is_unchanged(diff: &BundleDiff) -> bool {
    all_changes(diff).is_empty()
}

pub fn count_changes(changes: &[BundleChange]) -> BundleChangeCounts {
    let count = |kind: BundleChangeKind| changes.iter().filter(|c| c.change == kind).count();

    BundleChangeCounts {
        added: count(BundleChangeKind::Added),
        changed: count(BundleChangeKind::Changed),
        removed: count(BundleChangeKind::Removed),
    }
}

fn opaque<T: std::fmt::Debug>(values: &[T]) -> JsonValue {
    JsonValue::String(format!("{values:?}"))
}

fn optional(value: Option<&str>) -> JsonValue {
    value.map_or(JsonValue::Null, |value| JsonValue::String(value.to_owned()))
}

fn string_map(map: &indexmap::IndexMap<String, String>) -> JsonValue {
    JsonValue::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), JsonValue::String(value.clone())))
            .collect(),
    )
}

fn transport_shape(transport: &McpTransport) -> JsonValue {
    match transport {
        McpTransport::Stdio(stdio) => json!({
            "stdio": {
                "args": stdio.args,
                "command": stdio.command,
                "env": string_map(&stdio.env),
            }
        }),
        McpTransport::Http(http) => json!({
            "http": {
                "bearer_token_env_var": optional(http.bearer_token_env_var.as_deref()),
                "headers": string_map(&http.headers),
                "url": http.url,
            }
        }),
    }
}

fn same_value(before: Option<&JsonValue>, after: Option<&JsonValue>) -> bool {
    let null = JsonValue::Null;

    stringify(before.unwrap_or(&null)) == stringify(after.unwrap_or(&null))
}

fn first_field_difference(before: &JsonObject, after: &JsonObject, prefix: &str) -> Option<String> {
    let mut keys: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect();
    keys.sort_by(|a, b| js_cmp(a, b));

    for key in keys {
        let left = before.get(key);
        let right = after.get(key);

        if same_value(left, right) {
            continue;
        }

        let at = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };

        if let (Some(JsonValue::Object(left)), Some(JsonValue::Object(right))) = (left, right) {
            return first_field_difference(left, right, &at);
        }

        return Some(at);
    }

    None
}

fn record(value: JsonValue) -> JsonObject {
    match value {
        JsonValue::Object(object) => object,
        _ => unreachable!("every shape is built as an object literal"),
    }
}

fn pack_shape(pack: &MergedPack, reason: &str) -> JsonObject {
    record(json!({
        "catalog": pack.catalog,
        "description": optional(pack.description.as_deref()),
        "reason": reason,
        "requires": opaque(&pack.requires),
    }))
}

fn skill_shape(skill: &MergedSkill, reason: &str) -> JsonObject {
    record(json!({
        "catalog": skill.catalog,
        "description": optional(skill.description.as_deref()),
        "expects": opaque(&skill.expects),
        "reason": reason,
        "requires": opaque(&skill.requires),
    }))
}

fn mcp_shape(mcp: &MergedMcp, reason: &str) -> JsonObject {
    record(json!({
        "catalog": mcp.catalog,
        "expects": opaque(&mcp.expects),
        "reason": reason,
        "transport": transport_shape(&mcp.transport),
    }))
}

fn hook_shape(hook: &MergedHook, reason: &str) -> JsonObject {
    record(json!({
        "catalog": hook.catalog,
        "command": hook.command,
        "description": optional(hook.description.as_deref()),
        "event": hook.event.as_str(),
        "expects": opaque(&hook.expects),
        "matcher": optional(hook.matcher.as_deref()),
        "reason": reason,
        "timeout": hook.timeout.map_or(JsonValue::Null, JsonValue::from),
        "type": hook.r#type.as_str(),
    }))
}

trait BundleEntity {
    fn name(&self) -> &str;

    fn bytes_directory(&self) -> Option<PathBuf> {
        None
    }
}

impl BundleEntity for MergedPack {
    fn name(&self) -> &str {
        &self.name
    }
}

impl BundleEntity for MergedSkill {
    fn name(&self) -> &str {
        &self.name
    }

    fn bytes_directory(&self) -> Option<PathBuf> {
        Some(crate::util::path::join(&self.catalog_root, &self.path))
    }
}

impl BundleEntity for MergedMcp {
    fn name(&self) -> &str {
        &self.name
    }
}

impl BundleEntity for MergedHook {
    fn name(&self) -> &str {
        &self.name
    }

    fn bytes_directory(&self) -> Option<PathBuf> {
        Some(crate::util::path::join(&self.catalog_root, &self.path))
    }
}

fn file_list(dir: &Path) -> Option<Vec<String>> {
    fn walk(current: &Path, relative: &str, found: &mut Vec<String>) -> std::io::Result<()> {
        for name in read_dir_names(current)? {
            let within = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let entry = current.join(&name);

            if lstat_kind(&entry)? == EntryKind::Dir {
                walk(&entry, &within, found)?;
            } else {
                found.push(within);
            }
        }

        Ok(())
    }

    let mut found = Vec::new();
    walk(dir, "", &mut found).ok()?;
    found.sort_by(|a, b| js_cmp(a, b));
    Some(found)
}

fn same_tree(before: &Path, after: &Path) -> bool {
    if before == after {
        return true;
    }

    let (Some(left), Some(right)) = (file_list(before), file_list(after)) else {
        return false;
    };

    if left != right {
        return false;
    }

    for relative in &left {
        match (
            std::fs::read(before.join(relative)),
            std::fs::read(after.join(relative)),
        ) {
            (Ok(a), Ok(b)) if a == b => {}
            _ => return false,
        }
    }

    true
}

pub fn hook_summary(hook: &MergedHook) -> String {
    let matched = hook
        .matcher
        .as_ref()
        .map_or_else(String::new, |matcher| format!(" {matcher}"));

    format!(
        "{}{matched} — runs {}",
        hook.event,
        hook_command(hook, SHARED_HOOKS_DIR)
    )
}

struct Namespace<'a, T> {
    kind: ItemKind,
    before: &'a [T],
    after: &'a [T],
    shape: fn(&T, &str) -> JsonObject,
    arrival: Option<fn(&T) -> String>,
}

fn diff_namespace<T: BundleEntity>(
    namespace: &Namespace<'_, T>,
    before: &Bundle,
    after: &Bundle,
) -> Result<Vec<BundleChange>> {
    let was: indexmap::IndexMap<&str, &T> = namespace
        .before
        .iter()
        .map(|item| (item.name(), item))
        .collect();
    let is: indexmap::IndexMap<&str, &T> = namespace
        .after
        .iter()
        .map(|item| (item.name(), item))
        .collect();
    let mut names: Vec<&str> = was
        .keys()
        .chain(is.keys())
        .copied()
        .collect::<IndexSet<_>>()
        .into_iter()
        .collect();
    names.sort_by(|a, b| js_cmp(a, b));

    let mut changes = Vec::new();

    for name in names {
        let item = BundleItem {
            kind: namespace.kind,
            name: name.to_owned(),
        };
        let change = |change: BundleChangeKind, detail: String| BundleChange {
            kind: namespace.kind,
            name: name.to_owned(),
            change,
            detail,
        };

        match (was.get(name), is.get(name)) {
            (None, Some(right)) => {
                let detail = match namespace.arrival {
                    Some(arrival) => arrival(right),
                    None => format_reason(reason_of(after, &item)?),
                };

                changes.push(change(BundleChangeKind::Added, detail));
            }
            (Some(_), None) => {
                let detail = format!("was {}", format_reason(reason_of(before, &item)?));

                changes.push(change(BundleChangeKind::Removed, detail));
            }
            (Some(left), Some(right)) => {
                let field = first_field_difference(
                    &(namespace.shape)(left, &format_reason(reason_of(before, &item)?)),
                    &(namespace.shape)(right, &format_reason(reason_of(after, &item)?)),
                    "",
                );

                if let Some(field) = field {
                    changes.push(change(
                        BundleChangeKind::Changed,
                        format!("{field} changed"),
                    ));
                    continue;
                }

                if let (Some(from), Some(to)) = (left.bytes_directory(), right.bytes_directory())
                    && !same_tree(&from, &to)
                {
                    let detail = if namespace.kind == ItemKind::Hook {
                        "script changed"
                    } else {
                        "content changed"
                    };

                    changes.push(change(BundleChangeKind::Changed, detail.to_owned()));
                }
            }
            (None, None) => {}
        }
    }

    Ok(changes)
}

pub fn diff_bundles(before: &Bundle, after: &Bundle) -> Result<BundleDiff> {
    Ok(BundleDiff {
        packs: diff_namespace(
            &Namespace {
                kind: ItemKind::Pack,
                before: &before.packs,
                after: &after.packs,
                shape: pack_shape,
                arrival: None,
            },
            before,
            after,
        )?,
        skills: diff_namespace(
            &Namespace {
                kind: ItemKind::Skill,
                before: &before.skills,
                after: &after.skills,
                shape: skill_shape,
                arrival: None,
            },
            before,
            after,
        )?,
        mcps: diff_namespace(
            &Namespace {
                kind: ItemKind::Mcp,
                before: &before.mcps,
                after: &after.mcps,
                shape: mcp_shape,
                arrival: None,
            },
            before,
            after,
        )?,
        hooks: diff_namespace(
            &Namespace {
                kind: ItemKind::Hook,
                before: &before.hooks,
                after: &after.hooks,
                shape: hook_shape,
                arrival: Some(hook_summary),
            },
            before,
            after,
        )?,
    })
}
