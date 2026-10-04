//! A harness, described declaratively, and the one adapter that serves all of them.
//!
//! Installing a bundle works the same way for every agent tool: skills, and the scripts hooks
//! ship, are directories that get copied or symlinked; servers and hooks are entries merged into a
//! config file ambit co-owns; everything is planned before anything is written. What differs per
//! harness is exactly the fields of a profile: whether it needs a link to the skills directory,
//! which files its servers and hooks live in, which section of each, and what one server and one
//! hook look like there.
//!
//! So there is one implementation and one description per harness, not one implementation per
//! harness. A new harness is a profile; adding one should not require editing this file.

use std::path::Path;

use indexmap::IndexSet;

use crate::errors::{Result, config_error};
use crate::harness::adapter::{
    AppliedArtifact, HarnessAdapter, HookSkipReason, InstallScope, PlannedArtifact,
    PlannedCatalogDir, PlannedHarnessConfig, PlannedHookDir, PlannedSkillDir, PlannedSkillsLink,
    ProjectPaths, SkippedHook,
};
use crate::model::catalog::{MergedHook, MergedMcp, MergedSkill};
use crate::model::documents::{
    ConfigEntry, DocumentFormat, DocumentShape, JsonObject, array_entry_key, driver_for,
    managed_key, read_document_text,
};
use crate::model::hook_entity::{HookEvent, HookType};
use crate::model::state::{ArtifactKind, ArtifactMode, OwnedArtifact, State, owned_paths};
use crate::resolution::resolve::Bundle;
use crate::util::fs;
use crate::util::hash::tree_digest;
use crate::util::json::JsonValue;
use crate::util::path::{join, relative};

/// The directory the shared skills layout lives under, project-relative.
///
/// Named separately from [`SHARED_SKILLS_DIR`] because it is also the directory whose own
/// `.gitignore` lists what ambit installed there (see `project/gitignore.rs`).
pub const SHARED_AGENTS_DIR: &str = ".agents";

/// Where every harness's skills are materialized, project-relative.
///
/// One location for all of them: most harnesses read it natively, and the rest are pointed at it
/// with a link. A directory per harness would materialize the same skill several times in one
/// project.
pub const SHARED_SKILLS_DIR: &str = ".agents/skills";

/// Where the script a hook ships is materialized, project-relative.
///
/// Beside the skills directory for the same reason: one location for however many harnesses read
/// it, under the directory whose `.gitignore` lists what ambit put there.
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
/// `shape` picks the driver, since format alone cannot (`.mcp.json` and `.claude/settings.json`
/// are both JSON, and Kiro's hooks file holds one flat list rather than one array per event).
/// `root_defaults` names the keys a harness expects beside its hooks in a file ambit may create.
/// `events` says how the harness spells each event.
#[derive(Clone, Debug)]
pub struct HookLayout {
    /// Project-relative path to the config file.
    pub file: &'static str,
    /// The top-level key holding the hooks: one array per event, or one flat list.
    pub section: &'static str,
    /// How that file is parsed and written.
    pub format: DocumentFormat,
    /// How that section is laid out: `Array` for one array per event, `List` for one flat list
    /// whose entries name their own event.
    pub shape: DocumentShape,
    /// Root keys the file should carry beside its hooks: Cursor's `version: 1`, Kiro's
    /// `version: "v1"`.
    ///
    /// The driver seeds them only where the document lacks the key, so ambit adds one when
    /// creating the file and never overwrites a value someone else wrote. Only a merge applies
    /// them: pruning takes entries out and adds no keys.
    pub root_defaults: Option<JsonObject>,
    /// How this harness spells each event, where it differs from ambit's own spelling, and `None`
    /// for an event it has no counterpart for.
    ///
    /// Absent means Claude's `PascalCase` verbatim for every event. It lives on the layout, not
    /// the renderer, because it names which array an entry joins and which key it is recorded
    /// under. A function rather than a table, so a `match` keeps it total over [`HookEvent`] and
    /// every missing counterpart is written out as one.
    pub events: Option<fn(HookEvent) -> Option<&'static str>>,
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
    /// path is spelled: a documented placeholder where the harness has one, project-relative where
    /// it does not (`harness/definitions.rs`).
    ///
    /// Takes the [`MergedHook`] because that is what the planner holds. The rewrite only needs
    /// `type` (whether `command` is a path to rewrite or a command line to leave as written) and
    /// `name` (the directory the script was materialized under).
    pub hook_config: Option<fn(&MergedHook, &ProjectPaths) -> JsonValue>,
}

/// Which mode one materialized directory is written in.
///
/// A source pinned to a commit is immutable and gets copied; a source without one is a working
/// directory and gets linked. `commit` is absent exactly for a `path:` source, so no second notion
/// of "is this local" is needed.
///
/// Takes the one field it reads rather than a [`MergedSkill`], because a hook that ships a script
/// answers the same question the same way.
fn mode_of(commit: Option<&String>, project: &ProjectPaths) -> ArtifactMode {
    if let Some(mode) = project.mode {
        return mode;
    }

    if commit.is_none() {
        ArtifactMode::Link
    } else {
        ArtifactMode::Copy
    }
}

fn plan_skill(skill: &MergedSkill, project: &ProjectPaths) -> PlannedSkillDir {
    let relative = format!("{SHARED_SKILLS_DIR}/{}", skill.name);

    PlannedSkillDir {
        target: join(&project.root, &relative),
        path: relative,
        source: join(&skill.catalog_root, &skill.path),
        mode: mode_of(skill.commit.as_ref(), project),
        name: skill.name.clone(),
    }
}

/// The link, or nothing.
///
/// Nothing for a harness that reads the shared directory natively, and nothing for an empty
/// bundle: a project that selected no skills should not acquire a skills directory or a link to
/// one.
fn plan_skills_link(
    profile: &HarnessProfile,
    skills: &[MergedSkill],
    project: &ProjectPaths,
) -> Option<PlannedSkillsLink> {
    let link = profile.skills_link?;

    if skills.is_empty() {
        return None;
    }

    Some(PlannedSkillsLink {
        path: link.to_owned(),
        target: join(&project.root, link),
        source: join(&project.root, SHARED_SKILLS_DIR),
    })
}

/// The MCP config artifact, or nothing when the bundle selected no servers.
///
/// A bundle with no MCPs plans no artifact rather than an empty section, so a project that never
/// uses servers does not acquire a config file it did not ask for.
fn plan_mcp_config(
    profile: &HarnessProfile,
    mcps: &[MergedMcp],
    project: &ProjectPaths,
) -> Option<PlannedHarnessConfig> {
    if mcps.is_empty() {
        return None;
    }

    // `mcps` arrives sorted by name, so the entries, and the managed keys state records, are too.
    let entries: Vec<ConfigEntry> = mcps
        .iter()
        .map(|mcp| ConfigEntry {
            key: mcp.name.clone(),
            value: (profile.server_config)(mcp),
        })
        .collect();

    let file = if project.scope == Some(InstallScope::User) {
        profile.mcp.user_file.unwrap_or(profile.mcp.file)
    } else {
        profile.mcp.file
    };

    Some(PlannedHarnessConfig {
        path: file.to_owned(),
        target: join(&project.root, file),
        section: profile.mcp.section.to_owned(),
        format: profile.mcp.format,
        shape: None,
        root_defaults: None,
        managed_keys: entries
            .iter()
            .map(|entry| managed_key(profile.mcp.section, &entry.key))
            .collect(),
        entries,
    })
}

/// Which array in this harness's file one hook joins, or `None` for a hook it cannot express.
///
/// The single predicate behind both halves of the answer: [`plan_hook_config`] writes what this
/// names, and [`skipped_hooks`] reports what it does not. "Installed" and "skipped" partition the
/// bundle's hooks from the same fields rather than being computed twice.
///
/// An event [`HookLayout::events`] has no spelling for is a skip, not a fallback to ambit's own
/// spelling: a hook silently landing in an array the harness never reads would be worse.
fn hook_array_for(profile: &HarnessProfile, event: HookEvent) -> Option<&'static str> {
    let layout = profile.hooks.as_ref()?;

    profile.hook_config?;

    match layout.events {
        None => Some(event.as_str()),
        Some(spell) => spell(event),
    }
}

/// The hooks one harness was handed and cannot write.
///
/// Pure, and separate from the plan: nothing is written for these, they are only reported, and
/// the run succeeds. A harness that expresses no hooks at all reports every hook in the bundle; one
/// that expresses them reports only the events it has no spelling for.
pub fn skipped_hooks(profile: &HarnessProfile, hooks: &[MergedHook]) -> Vec<SkippedHook> {
    let reason = if profile.hooks.is_none() {
        HookSkipReason::NoMechanism
    } else {
        HookSkipReason::NoEvent
    };

    hooks
        .iter()
        .filter(|hook| hook_array_for(profile, hook.event).is_none())
        .map(|hook| SkippedHook {
            harness: profile.name.to_owned(),
            hook: hook.name.clone(),
            event: hook.event,
            reason,
        })
        .collect()
}

/// The directory one hook's script is materialized from, or nothing.
///
/// Nothing for two cases: a hook whose `command` is a command line (most of them), and a hook this
/// harness cannot express at all. Both use the same predicate [`plan_hook_config`] and
/// [`skipped_hooks`] partition the bundle with, so a script is never installed for a harness that
/// skipped the hook. A project on opencode alone acquires no `.agents/hooks` at all.
///
/// `type` is the whole test: every hook comes out of a catalog directory, so `catalog_root` and
/// `path` are always there to build the source from. A `command` hook lacks bytes, not a location
/// for them.
fn plan_hook_dir(
    profile: &HarnessProfile,
    hook: &MergedHook,
    project: &ProjectPaths,
) -> Option<PlannedHookDir> {
    if hook.r#type != HookType::Script {
        return None;
    }

    hook_array_for(profile, hook.event)?;

    let relative = format!("{SHARED_HOOKS_DIR}/{}", hook.name);

    Some(PlannedHookDir {
        target: join(&project.root, &relative),
        path: relative,
        source: join(&hook.catalog_root, &hook.path),
        mode: mode_of(hook.commit.as_ref(), project),
        name: hook.name.clone(),
    })
}

/// The hooks config artifact, or nothing.
///
/// Nothing for a harness with no hook mechanism, and nothing for a bundle that selected no hooks,
/// for the same reason as [`plan_mcp_config`]: a project that declares no hooks should not acquire
/// a settings file it never asked for. Also nothing when this harness must skip every hook in the
/// bundle, since the file would hold an empty section nobody asked for.
///
/// The key is the entry's own content digest, since an event's array carries no name to key on.
/// The value is rendered once, and both the key and the entry are read off that one rendering, so
/// the digest always names the bytes actually written.
///
/// The key also carries the event as this harness spells it: it must name the array the entry
/// actually sits in, or `section_keys` reading the file back would not recognize what ambit wrote,
/// and every install would append the hook again.
fn plan_hook_config(
    profile: &HarnessProfile,
    hooks: &[MergedHook],
    project: &ProjectPaths,
) -> Option<PlannedHarnessConfig> {
    let layout = profile.hooks.as_ref()?;
    let render = profile.hook_config?;

    // `hooks` arrives sorted by name, so the entries, and the managed keys state records, are too.
    let entries: Vec<ConfigEntry> = hooks
        .iter()
        .filter_map(|hook| {
            let event = hook_array_for(profile, hook.event)?;
            let value = render(hook, project);

            Some(ConfigEntry {
                key: array_entry_key(event, &value),
                value,
            })
        })
        .collect();

    if entries.is_empty() {
        return None;
    }

    Some(PlannedHarnessConfig {
        path: layout.file.to_owned(),
        target: join(&project.root, layout.file),
        section: layout.section.to_owned(),
        format: layout.format,
        shape: Some(layout.shape),
        root_defaults: layout.root_defaults.clone(),
        managed_keys: entries
            .iter()
            .map(|entry| managed_key(layout.section, &entry.key))
            .collect(),
        entries,
    })
}

/// Writes a relative symlink.
///
/// Relative, because a project and the catalog it points at are often one checkout: a relative
/// link survives the tree being moved, and it keeps a machine-specific absolute path out of the
/// working copy. `readlink` then shows a reader the same thing `ambit status` compares.
///
/// # Errors
///
/// Exit 2 when the link cannot be created: something already at the target, which every install
/// path has already refused or removed, or a filesystem that will not make symlinks.
fn link(from: &Path, at: &Path, label: &str, hint: &str) -> Result<()> {
    let parent = at.parent().unwrap_or_else(|| Path::new(""));
    let target = relative(parent, from);

    // A directory link is what Windows needs to link a directory; POSIX ignores the distinction.
    fs::symlink_dir(Path::new(&target), at).map_err(|error| {
        config_error(
            format!("cannot symlink {label}"),
            [fs::io_message(&error, at), hint.to_owned()],
        )
    })
}

/// Creates the directory `target` sits in.
fn mkdir_parent(target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::mkdir_p(parent)?;
    }

    Ok(())
}

/// Writes one directory out of a catalog (a skill's, or the script a hook ships) in the mode the
/// plan chose.
///
/// One function over both kinds, since a hook's directory is materialized under exactly the same
/// rules: everything below is a statement about a directory ambit owns, not about what it holds.
///
/// An owned target is removed before being rewritten, so a skill that lost a file upstream does
/// not keep a stale copy of it, and a directory whose mode changed between runs becomes the other
/// thing rather than a copy sitting on top of a link. An unowned target is copied over rather than
/// replaced: a case an install never reaches, since ownership enforcement has already refused it
/// or adopted it. It stays a merge anyway: `apply` called directly, with a state that claims
/// nothing, must not delete a stranger's directory.
///
/// A copy preserves each file's mode, so a hook script arrives executable if the catalog ships it
/// that way. A linked directory has no bytes of its own and needs nothing.
fn apply_catalog_dir(
    artifact: &PlannedCatalogDir,
    kind: ArtifactKind,
    owned: &IndexSet<String>,
) -> Result<AppliedArtifact> {
    if owned.contains(&artifact.path) {
        // A directory is removed whole; a symlink is unlinked without following it, so the link's
        // source is never deleted.
        fs::rm_rf(&artifact.target)?;
    }

    mkdir_parent(&artifact.target)?;

    if artifact.mode == ArtifactMode::Link {
        link(
            &artifact.source,
            &artifact.target,
            &artifact.path,
            &format!(
                "move {} aside, or run `ambit install --copy` to copy \"{}\" instead",
                artifact.path, artifact.name
            ),
        )?;
    } else {
        fs::copy_tree(&artifact.source, &artifact.target)?;
    }

    // Hashed from the copy just written rather than from its source, so `status` compares like
    // with like. The two agree unless the tree holds a link out of itself, which `copy_tree`
    // rewrites.
    let digest = match artifact.mode {
        ArtifactMode::Copy => Some(tree_digest(&artifact.target)?),
        ArtifactMode::Link => None,
    };

    Ok(OwnedArtifact {
        path: artifact.path.clone(),
        kind,
        mode: Some(artifact.mode),
        managed_keys: None,
        format: None,
        shape: None,
        digest,
    })
}

/// Points a harness's skills directory at the shared one.
///
/// The shared directory is created first even when the bundle is empty: a link to a directory that
/// does not exist is a dangling link, and a harness reading one reports a broken install rather
/// than an empty one.
fn apply_skills_link(
    artifact: &PlannedSkillsLink,
    owned: &IndexSet<String>,
) -> Result<AppliedArtifact> {
    fs::mkdir_p(&artifact.source)?;

    if owned.contains(&artifact.path) {
        fs::rm_rf(&artifact.target)?;
    }

    mkdir_parent(&artifact.target)?;

    link(
        &artifact.source,
        &artifact.target,
        &artifact.path,
        &format!(
            "move {} aside, so ambit can point it at {SHARED_SKILLS_DIR}",
            artifact.path
        ),
    )?;

    Ok(OwnedArtifact {
        path: artifact.path.clone(),
        kind: ArtifactKind::SkillsLink,
        mode: Some(ArtifactMode::Link),
        managed_keys: None,
        format: None,
        shape: None,
        digest: None,
    })
}

/// Merges the planned entries into the harness's config file.
///
/// Read-modify-write rather than a plain write, whether or not the file is owned: ambit owns keys
/// here, not the document, so a hand-maintained config is a normal input rather than a conflict.
///
/// The only site that passes `root_defaults`, since it is the only one writing a document rather
/// than reading or emptying one. Prune, clean, and status all build their driver from state, which
/// records the shape and no defaults.
fn apply_harness_config(artifact: &PlannedHarnessConfig) -> Result<AppliedArtifact> {
    let driver = driver_for(
        artifact.format,
        artifact.shape.unwrap_or(DocumentShape::Map),
        artifact.root_defaults.as_ref(),
    )?;
    let text = read_document_text(&artifact.target, &artifact.path)?;
    let merged = driver.merge_section(
        text.as_deref(),
        &artifact.section,
        &artifact.entries,
        &artifact.path,
    )?;

    mkdir_parent(&artifact.target)?;
    fs::write_text(&artifact.target, &merged)?;

    Ok(OwnedArtifact {
        path: artifact.path.clone(),
        kind: ArtifactKind::HarnessConfig,
        mode: None,
        managed_keys: Some(artifact.managed_keys.clone()),
        format: Some(artifact.format),
        shape: artifact.shape,
        digest: None,
    })
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
    #[cfg(test)]
    fn name(&self) -> &str {
        self.profile.name
    }

    /// Every list on the bundle is already sorted by name, so the plan is too.
    fn plan(&self, bundle: &Bundle, project: &ProjectPaths) -> Vec<PlannedArtifact> {
        let profile = self.profile;
        let mut plan: Vec<PlannedArtifact> = bundle
            .skills
            .iter()
            .map(|skill| PlannedArtifact::SkillDir(plan_skill(skill, project)))
            .collect();

        // Directories before configs. Most hooks plan none: a hook with no script is just a
        // command line, which is the config artifact's business.
        plan.extend(
            bundle
                .hooks
                .iter()
                .filter_map(|hook| plan_hook_dir(profile, hook, project))
                .map(PlannedArtifact::HookDir),
        );
        plan.extend(
            plan_skills_link(profile, &bundle.skills, project).map(PlannedArtifact::SkillsLink),
        );
        plan.extend(
            plan_mcp_config(profile, &bundle.mcps, project).map(PlannedArtifact::HarnessConfig),
        );
        plan.extend(
            plan_hook_config(profile, &bundle.hooks, project).map(PlannedArtifact::HarnessConfig),
        );
        plan
    }

    fn skips(&self, bundle: &Bundle) -> Vec<SkippedHook> {
        skipped_hooks(self.profile, &bundle.hooks)
    }

    fn apply(&self, plan: &[PlannedArtifact], prior: &State) -> Result<Vec<AppliedArtifact>> {
        let owned = owned_paths(prior);
        let mut applied = Vec::with_capacity(plan.len());

        for artifact in plan {
            // Both directory kinds are named explicitly: falling through to the config arm would
            // try to merge a section into a directory.
            applied.push(match artifact {
                PlannedArtifact::SkillDir(dir) | PlannedArtifact::HookDir(dir) => {
                    apply_catalog_dir(dir, artifact.kind(), &owned)?
                }
                PlannedArtifact::SkillsLink(link) => apply_skills_link(link, &owned)?,
                PlannedArtifact::HarnessConfig(config) => apply_harness_config(config)?,
            });
        }

        Ok(applied)
    }
}

/// Whether every entry in a directory is a path ambit already owns.
///
/// Makes replacing an old-layout `.claude/skills` safe: if ambit created everything inside it,
/// turning it into a link to the shared directory loses nothing. A single unowned entry (one
/// hand-written skill) makes the answer no.
///
/// A directory that cannot be listed answers no too, so the caller's refusal stands.
///
/// # Errors
///
/// None today: an unlistable directory is a `false`, not an error. The `Result` is the contract
/// other batches code against, kept so an error can be added without touching callers.
#[allow(clippy::unnecessary_wraps)]
pub fn holds_only_owned(target: &Path, relative: &str, owned: &IndexSet<String>) -> Result<bool> {
    let Ok(entries) = fs::read_dir_names(target) else {
        return Ok(false);
    };

    Ok(entries
        .iter()
        .all(|entry| owned.contains(&format!("{relative}/{entry}"))))
}
