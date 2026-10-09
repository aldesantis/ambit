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

pub const SHARED_AGENTS_DIR: &str = ".agents";

pub const SHARED_SKILLS_DIR: &str = ".agents/skills";

pub const SHARED_HOOKS_DIR: &str = ".agents/hooks";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpLayout {
    pub file: &'static str,
    pub user_file: Option<&'static str>,
    pub section: &'static str,
    pub format: DocumentFormat,
}

#[derive(Clone, Debug)]
pub struct HookLayout {
    pub file: &'static str,
    pub section: &'static str,
    pub format: DocumentFormat,
    pub shape: DocumentShape,
    pub root_defaults: Option<JsonObject>,
    pub events: Option<fn(HookEvent) -> Option<&'static str>>,
}

#[derive(Clone, Debug)]
pub struct HarnessProfile {
    pub name: &'static str,
    pub skills_link: Option<&'static str>,
    pub mcp: McpLayout,
    pub server_config: fn(&MergedMcp) -> JsonValue,
    pub hooks: Option<HookLayout>,
    pub hook_config: Option<fn(&MergedHook, &ProjectPaths) -> JsonValue>,
}

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

fn plan_mcp_config(
    profile: &HarnessProfile,
    mcps: &[MergedMcp],
    project: &ProjectPaths,
) -> Option<PlannedHarnessConfig> {
    if mcps.is_empty() {
        return None;
    }

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

fn hook_array_for(profile: &HarnessProfile, event: HookEvent) -> Option<&'static str> {
    let layout = profile.hooks.as_ref()?;

    profile.hook_config?;

    match layout.events {
        None => Some(event.as_str()),
        Some(spell) => spell(event),
    }
}

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

fn plan_hook_config(
    profile: &HarnessProfile,
    hooks: &[MergedHook],
    project: &ProjectPaths,
) -> Option<PlannedHarnessConfig> {
    let layout = profile.hooks.as_ref()?;
    let render = profile.hook_config?;

    let entries: Vec<ConfigEntry> = hooks
        .iter()
        .filter_map(|hook| {
            let event = hook_array_for(profile, hook.event)?;
            let value = render(hook, project);

            Some(ConfigEntry {
                // Must use the harness's own event spelling, or reading the file back misses the
                // entry and every install appends it again.
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

fn link(from: &Path, at: &Path, label: &str, hint: &str) -> Result<()> {
    let parent = at.parent().unwrap_or_else(|| Path::new(""));
    let target = relative(parent, from);

    fs::symlink_dir(Path::new(&target), at).map_err(|error| {
        config_error(
            format!("cannot symlink {label}"),
            [fs::io_message(&error, at), hint.to_owned()],
        )
    })
}

fn mkdir_parent(target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::mkdir_p(parent)?;
    }

    Ok(())
}

fn apply_catalog_dir(
    artifact: &PlannedCatalogDir,
    kind: ArtifactKind,
    owned: &IndexSet<String>,
) -> Result<AppliedArtifact> {
    if owned.contains(&artifact.path) {
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

    // Hashed from the copy, not the source: `copy_tree` rewrites links that leave the tree.
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

fn apply_skills_link(
    artifact: &PlannedSkillsLink,
    owned: &IndexSet<String>,
) -> Result<AppliedArtifact> {
    // Created even for an empty bundle: a dangling link reads to the harness as a broken install.
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

#[derive(Clone, Copy, Debug)]
pub struct ProfileAdapter {
    pub profile: &'static HarnessProfile,
}

pub fn adapter_for(profile: &'static HarnessProfile) -> ProfileAdapter {
    ProfileAdapter { profile }
}

impl HarnessAdapter for ProfileAdapter {
    #[cfg(test)]
    fn name(&self) -> &str {
        self.profile.name
    }

    fn plan(&self, bundle: &Bundle, project: &ProjectPaths) -> Vec<PlannedArtifact> {
        let profile = self.profile;
        let mut plan: Vec<PlannedArtifact> = bundle
            .skills
            .iter()
            .map(|skill| PlannedArtifact::SkillDir(plan_skill(skill, project)))
            .collect();

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

#[allow(clippy::unnecessary_wraps)]
pub fn holds_only_owned(target: &Path, relative: &str, owned: &IndexSet<String>) -> Result<bool> {
    let Ok(entries) = fs::read_dir_names(target) else {
        return Ok(false);
    };

    Ok(entries
        .iter()
        .all(|entry| owned.contains(&format!("{relative}/{entry}"))))
}
