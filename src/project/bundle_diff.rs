//! Comparing two resolutions of one project, for `outdated` and `update`.

use crate::errors::Result;
use crate::model::catalog::MergedHook;
use crate::resolution::resolve::{Bundle, ItemKind};
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
    /// One line a reader can act on: why it is here now, why it was here, or what moved. Never
    /// empty.
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
    let _ = diff;
    todo!("port project/bundle-diff.ts:allChanges")
}

/// Whether the two bundles are the same bundle: the answer a report leads with.
pub fn is_unchanged(diff: &BundleDiff) -> bool {
    let _ = diff;
    todo!("port project/bundle-diff.ts:isUnchanged")
}

pub fn count_changes(changes: &[BundleChange]) -> BundleChangeCounts {
    let _ = changes;
    todo!("port project/bundle-diff.ts:countChanges")
}

/// How a hook reads in a report: the event it fires on, what filters it, and what it will run.
///
/// Uses the command as the harness will receive it, so a hook shipping a script names the
/// installed path, not the catalog-relative filename the author wrote.
pub fn hook_summary(hook: &MergedHook) -> String {
    let _ = hook;
    todo!("port project/bundle-diff.ts:hookSummary")
}

/// Compares two resolutions of one project: `before` is the bundle the project resolves to now,
/// `after` the one it would resolve to with the pins moved.
///
/// # Errors
///
/// Exit 2 when a skill or hook directory cannot be read for the byte comparison.
pub fn diff_bundles(before: &Bundle, after: &Bundle) -> Result<BundleDiff> {
    let _ = (before, after);
    todo!("port project/bundle-diff.ts:diffBundles")
}
