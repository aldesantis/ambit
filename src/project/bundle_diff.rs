//! Comparing two bundles: what `ambit outdated` and `ambit update` report instead of two commit
//! SHAs.
//!
//! `resolve_bundle` is pure over `(config, merged_catalog)`, so a project's catalogs can be resolved
//! at two different commits in one process and the two bundles compared directly. The report
//! answers "what new thing will run in my session", not "which commit", so it lists gained skills,
//! changed servers, and added hooks rather than a SHA.
//!
//! A catalog whose branch advanced past commits that touched nothing this project selects produces
//! an empty diff. Nothing here reads a commit directly: an item is compared by what it declares and
//! by the bytes it ships.
//!
//! Fields are compared before content. `description changed` and `requires changed` both also
//! change a `SKILL.md`'s bytes, but naming the field is more useful than naming the file, so a
//! declared difference is reported first and "content changed" is reported only when no field
//! moved.
//!
//! The comparison is of the merged item, not of the catalog's copy: `catalog` and the selection
//! reason are compared alongside everything an entity declares, since a name moving between
//! catalogs, or a skill now reached through a pack instead of directly, is a real change to what the
//! project got.
//!
//! This is not what [`assert_lock_current`](crate::project::lock::assert_lock_current) does.
//! `--frozen` compares the lock as bytes; see `project/lock.rs`.

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
    /// What happened to one name between two bundles.
    ///
    /// Three states, not the five `status` reports: this compares two resolutions of the same
    /// project rather than a resolution against disk, so there is no ownership to judge and nothing
    /// can be missing.
    pub enum BundleChangeKind {
        Added => "added",
        Changed => "changed",
        Removed => "removed",
    }
}

/// Every change kind, in declaration order.
pub const BUNDLE_CHANGE_KINDS: &[BundleChangeKind] = BundleChangeKind::ALL;

/// One item that entered, left, or changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleChange {
    pub kind: ItemKind,
    pub name: String,
    pub change: BundleChangeKind,
    /// One line a reader can act on: why it is here now, why it was here, or what moved.
    ///
    /// Never empty. A row that only says something changed would send the reader to `git log`,
    /// which this report exists to replace.
    pub detail: String,
}

/// Two bundles compared, one list per namespace, each sorted by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleDiff {
    pub packs: Vec<BundleChange>,
    pub skills: Vec<BundleChange>,
    pub mcps: Vec<BundleChange>,
    pub hooks: Vec<BundleChange>,
}

/// How many of each kind one list holds: the `+2 ~1 -0` a report puts beside a namespace.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BundleChangeCounts {
    pub added: usize,
    pub changed: usize,
    pub removed: usize,
}

/// Every change across the four namespaces, in the order a report prints them.
pub fn all_changes(diff: &BundleDiff) -> Vec<BundleChange> {
    diff.packs
        .iter()
        .chain(&diff.skills)
        .chain(&diff.mcps)
        .chain(&diff.hooks)
        .cloned()
        .collect()
}

/// Whether the two bundles are the same bundle: the answer a report leads with.
pub fn is_unchanged(diff: &BundleDiff) -> bool {
    diff.packs.is_empty() && diff.skills.is_empty() && diff.mcps.is_empty() && diff.hooks.is_empty()
}

pub fn count_changes(changes: &[BundleChange]) -> BundleChangeCounts {
    let count = |kind: BundleChangeKind| changes.iter().filter(|c| c.change == kind).count();

    BundleChangeCounts {
        added: count(BundleChangeKind::Added),
        changed: count(BundleChangeKind::Changed),
        removed: count(BundleChangeKind::Removed),
    }
}

/// An array of values the walk never descends into, as one opaque comparable value.
///
/// Arrays compare whole: an index is not a field name, and `env[1] changed` tells a reader less
/// than `env changed` while sounding more precise. The rendering is only ever compared, never
/// printed, so any rendering that is equal exactly when the values are equal will do.
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

/// The transport as the entity's own document writes it, so a difference reads
/// `transport.http.url` rather than `transport.url`: the path a reader is given should match the
/// file.
///
/// The kind key is also the discriminator, so a server that changed from stdio to http differs at
/// `transport` itself.
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

/// Whether two values a projection can hold are the same value. An absent key reads as `null`.
fn same_value(before: Option<&JsonValue>, after: Option<&JsonValue>) -> bool {
    let null = JsonValue::Null;

    stringify(before.unwrap_or(&null)) == stringify(after.unwrap_or(&null))
}

/// The dotted path of the first field two projections disagree about, or `None` when they agree.
///
/// Keys are visited in sorted order, not declaration order, so the same pair of items always
/// reports the same field. Only a plain object is descended into, since its keys are field names;
/// an array is compared whole, per [`opaque`].
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

        // Both sides present and both records: recurse to name the innermost differing field. One
        // side absent is a difference at this level.
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

/// What a pack declares, as one comparable record.
///
/// `requires` is the whole of what a pack does, so a membership change reports as
/// `requires changed`. What was gained or lost shows up as its own rows in the other three
/// sections.
fn pack_shape(pack: &MergedPack, reason: &str) -> JsonObject {
    record(json!({
        "catalog": pack.catalog,
        "description": optional(pack.description.as_deref()),
        "reason": reason,
        "requires": opaque(&pack.requires),
    }))
}

/// What a skill declares, as one comparable record.
fn skill_shape(skill: &MergedSkill, reason: &str) -> JsonObject {
    record(json!({
        "catalog": skill.catalog,
        "description": optional(skill.description.as_deref()),
        "expects": opaque(&skill.expects),
        "reason": reason,
        "requires": opaque(&skill.requires),
    }))
}

/// What a server declares, as one comparable record.
fn mcp_shape(mcp: &MergedMcp, reason: &str) -> JsonObject {
    record(json!({
        "catalog": mcp.catalog,
        "expects": opaque(&mcp.expects),
        "reason": reason,
        "transport": transport_shape(&mcp.transport),
    }))
}

/// What a hook declares, as one comparable record.
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

/// What the namespaces have in common: a name, and, for kinds that can ship bytes, where those
/// bytes are.
trait BundleEntity {
    fn name(&self) -> &str;

    /// The directory the item's bytes live in, or `None` when it has none.
    ///
    /// A server has none: it is config values in a document, and each one is already compared as a
    /// field. A pack ships nothing at all.
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

/// Every file under `dir`, relative, `/`-separated and sorted, or `None` when it cannot be read.
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

/// Whether two directories hold the same files with the same bytes.
///
/// A tree that cannot be read counts as differing, not as an error, the same way `status` treats an
/// unreadable file: a report of what an update would bring should not fail over a permission
/// problem.
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

/// How a hook reads in a report: the event it fires on, what filters it, and what it will run.
///
/// Uses the command as the harness will receive it, so a hook shipping a script names the
/// installed path, not the catalog-relative filename the author wrote: the string that will
/// actually execute.
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

/// What one namespace's comparison needs to know about the kind it is comparing.
struct Namespace<'a, T> {
    kind: ItemKind,
    before: &'a [T],
    after: &'a [T],
    /// The comparable projection of one item, given the reason its own bundle gives for it.
    shape: fn(&T, &str) -> JsonObject,
    /// How an arriving item introduces itself. `None` means the reason it was selected for.
    arrival: Option<fn(&T) -> String>,
}

/// One namespace compared.
///
/// Built from the union of both sides' names, sorted, so the report is a function of the two
/// bundles, not of assembly order.
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
                // Why it was there; a reader checks this against their config to decide whether
                // losing it was intended.
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
                    // Named for what the item is: a skill is a directory of instructions, a hook
                    // that ships bytes is a script.
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

/// Compares two resolutions of one project: `before` is the bundle the project resolves to now,
/// `after` the one it would resolve to with the pins moved.
///
/// Everything a report needs beyond "did the files change" is already in the two bundles; only the
/// byte comparison touches disk.
///
/// # Errors
///
/// Exit 1 when a bundle holds an item it has no selection reason for, which is a resolver bug.
/// An unreadable skill or hook directory is not an error: it reads as changed.
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
                // A hook's arrival means something starts executing, so unlike other kinds it says
                // what, instead of just naming the reason it was selected.
                arrival: Some(hook_summary),
            },
            before,
            after,
        )?,
    })
}
