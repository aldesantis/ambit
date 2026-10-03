//! Harness profiles, and the one adapter implementation every profile shares.

use std::path::Path;

use indexmap::IndexSet;

use crate::errors::Result;
use crate::harness::adapter::{
    AppliedArtifact, HarnessAdapter, PlannedArtifact, ProjectPaths, SkippedHook,
};
use crate::model::catalog::{MergedHook, MergedMcp};
use crate::model::documents::{DocumentFormat, DocumentShape, JsonObject};
use crate::model::hook_entity::HookEvent;
use crate::model::state::State;
use crate::resolution::resolve::Bundle;
use crate::util::json::JsonValue;

/// The directory the shared skills layout lives under, project-relative.
///
/// Named separately from [`SHARED_SKILLS_DIR`] because it is also the directory whose own
/// `.gitignore` lists what ambit installed there (see `project/gitignore.rs`).
pub const SHARED_AGENTS_DIR: &str = ".agents";

/// Where every harness's skills are materialized, project-relative.
///
/// One location for all of them: three of the five harnesses read it natively, and the other two
/// are pointed at it with a link.
pub const SHARED_SKILLS_DIR: &str = ".agents/skills";

/// Where the script a hook ships is materialized, project-relative.
pub const SHARED_HOOKS_DIR: &str = ".agents/hooks";

/// Where a harness reads its MCP servers from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpLayout {
    /// Project-relative path to the config file.
    pub file: &'static str,
    /// Home-relative config file, when the harness stores user MCPs elsewhere.
    pub user_file: Option<&'static str>,
    /// The top-level key holding one entry per server.
    pub section: &'static str,
    /// How that file is parsed and written.
    pub format: DocumentFormat,
}

/// Where a harness reads its hooks from.
///
/// Three fields wider than [`McpLayout`], because a hooks section is not a table keyed by name.
#[derive(Clone, Debug)]
pub struct HookLayout {
    /// Project-relative path to the config file.
    pub file: &'static str,
    /// The top-level key holding one array per event.
    pub section: &'static str,
    /// How that file is parsed and written.
    pub format: DocumentFormat,
    /// How that section is laid out: `Array` for every harness that expresses hooks at all.
    pub shape: DocumentShape,
    /// Root keys the file should carry beside its hooks: Cursor's `version: 1`. Only a merge
    /// applies them.
    pub root_defaults: Option<JsonObject>,
    /// How this harness spells each event, where it differs from ambit's own spelling.
    ///
    /// Absent means Claude's `PascalCase` verbatim, which is what Claude, VS Code, and Codex read.
    /// Cursor is the one harness needing a map. A function rather than a table, so a `match` keeps
    /// it total over [`HookEvent`].
    pub events: Option<fn(HookEvent) -> &'static str>,
}

/// One agent tool's layout.
#[derive(Clone, Debug)]
pub struct HarnessProfile {
    /// The name `ambit.yml`'s `harnesses` uses.
    pub name: &'static str,
    /// A directory to symlink at [`SHARED_SKILLS_DIR`], for a harness that does not read it
    /// natively. Absent means the harness already looks in the shared location.
    pub skills_link: Option<&'static str>,
    pub mcp: McpLayout,
    /// One server, in this harness's own shape.
    ///
    /// The only genuinely harness-specific knowledge in the install path: that `http` means
    /// `type`/`url`/`headers` here and `type: "remote"` there, and how each spells a reference to
    /// an environment variable.
    pub server_config: fn(&MergedMcp) -> JsonValue,
    /// Where this harness's hooks live, or absent for one with no declarative hook mechanism.
    /// Declared together with `hook_config`: a profile carries both or neither.
    pub hooks: Option<HookLayout>,
    /// One hook, in this harness's own shape: the counterpart of `server_config`.
    ///
    /// Turns a neutral `PreToolUse` into whatever this harness spells it, decides what a `matcher`
    /// or `timeout` turns into, and, for a hook that ships a script, decides how the materialized
    /// path is spelled (`harness/definitions.rs`).
    pub hook_config: Option<fn(&MergedHook, &ProjectPaths) -> JsonValue>,
}

/// The hooks one harness was handed and cannot write.
///
/// Pure, and separate from the plan: nothing is written for these, they are only reported, and
/// the run succeeds. A harness that expresses no hooks at all reports every hook in the bundle; one
/// that expresses them reports only the events it has no spelling for.
pub fn skipped_hooks(profile: &HarnessProfile, hooks: &[MergedHook]) -> Vec<SkippedHook> {
    let _ = (profile, hooks);
    todo!("port harness/profile.ts:skippedHooks")
}

/// The adapter for one profile; see [`adapter_for`].
#[derive(Clone, Copy, Debug)]
pub struct ProfileAdapter {
    pub profile: &'static HarnessProfile,
}

/// Builds the adapter for one profile.
pub fn adapter_for(profile: &'static HarnessProfile) -> ProfileAdapter {
    ProfileAdapter { profile }
}

impl HarnessAdapter for ProfileAdapter {
    fn name(&self) -> &str {
        self.profile.name
    }

    fn plan(&self, bundle: &Bundle, project: &ProjectPaths) -> Vec<PlannedArtifact> {
        let _ = (bundle, project);
        todo!("port harness/profile.ts:adapterFor.plan")
    }

    fn skips(&self, bundle: &Bundle) -> Vec<SkippedHook> {
        skipped_hooks(self.profile, &bundle.hooks)
    }

    fn apply(&self, plan: &[PlannedArtifact], prior: &State) -> Result<Vec<AppliedArtifact>> {
        let _ = (plan, prior);
        todo!("port harness/profile.ts:adapterFor.apply")
    }
}

/// Whether every entry in a directory is a path ambit already owns.
///
/// Makes replacing an old-layout `.claude/skills` safe: if ambit created everything inside it,
/// turning it into a link to the shared directory loses nothing. A single unowned entry makes the
/// answer no.
///
/// # Errors
///
/// Exit 2 when the directory cannot be listed.
pub fn holds_only_owned(target: &Path, relative: &str, owned: &IndexSet<String>) -> Result<bool> {
    let _ = (target, relative, owned);
    todo!("port harness/profile.ts:holdsOnlyOwned")
}
