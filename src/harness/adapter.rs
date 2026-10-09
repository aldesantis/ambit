use std::path::{Path, PathBuf};

use crate::errors::Result;
use crate::model::documents::{ConfigEntry, DocumentFormat, DocumentShape, JsonObject};
use crate::model::hook_entity::HookEvent;
use crate::model::state::{ArtifactKind, ArtifactMode, OwnedArtifact, State};
use crate::resolution::resolve::Bundle;
use crate::util::string_enum;

string_enum! {
    pub enum InstallScope {
        Project => "project",
        User => "user",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectPaths {
    pub root: PathBuf,
    pub scope: Option<InstallScope>,
    pub mode: Option<ArtifactMode>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCatalogDir {
    pub path: String,
    pub target: PathBuf,
    pub source: PathBuf,
    pub mode: ArtifactMode,
    pub name: String,
}

pub type PlannedSkillDir = PlannedCatalogDir;

pub type PlannedHookDir = PlannedCatalogDir;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedSkillsLink {
    pub path: String,
    pub target: PathBuf,
    pub source: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlannedHarnessConfig {
    pub path: String,
    pub target: PathBuf,
    pub section: String,
    pub format: DocumentFormat,
    pub shape: Option<DocumentShape>,
    pub root_defaults: Option<JsonObject>,
    pub entries: Vec<ConfigEntry>,
    pub managed_keys: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PlannedArtifact {
    SkillDir(PlannedSkillDir),
    HookDir(PlannedHookDir),
    SkillsLink(PlannedSkillsLink),
    HarnessConfig(PlannedHarnessConfig),
}

impl PlannedArtifact {
    pub fn kind(&self) -> ArtifactKind {
        match self {
            Self::SkillDir(_) => ArtifactKind::SkillDir,
            Self::HookDir(_) => ArtifactKind::HookDir,
            Self::SkillsLink(_) => ArtifactKind::SkillsLink,
            Self::HarnessConfig(_) => ArtifactKind::HarnessConfig,
        }
    }

    pub fn path(&self) -> &str {
        match self {
            Self::SkillDir(dir) | Self::HookDir(dir) => &dir.path,
            Self::SkillsLink(link) => &link.path,
            Self::HarnessConfig(config) => &config.path,
        }
    }

    pub fn target(&self) -> &Path {
        match self {
            Self::SkillDir(dir) | Self::HookDir(dir) => &dir.target,
            Self::SkillsLink(link) => &link.target,
            Self::HarnessConfig(config) => &config.target,
        }
    }

    pub fn mode(&self) -> Option<ArtifactMode> {
        match self {
            Self::SkillDir(dir) | Self::HookDir(dir) => Some(dir.mode),
            Self::SkillsLink(_) => Some(ArtifactMode::Link),
            Self::HarnessConfig(_) => None,
        }
    }
}

impl From<&PlannedArtifact> for OwnedArtifact {
    fn from(artifact: &PlannedArtifact) -> Self {
        let config = match artifact {
            PlannedArtifact::HarnessConfig(config) => Some(config),
            _ => None,
        };

        Self {
            path: artifact.path().to_owned(),
            kind: artifact.kind(),
            mode: artifact.mode(),
            managed_keys: config.map(|config| config.managed_keys.clone()),
            format: config.map(|config| config.format),
            shape: config.and_then(|config| config.shape),
            digest: None,
        }
    }
}

pub type AppliedArtifact = OwnedArtifact;

string_enum! {
    pub enum HookSkipReason {
        NoMechanism => "no-mechanism",
        NoEvent => "no-event",
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedHook {
    pub harness: String,
    pub hook: String,
    pub event: HookEvent,
    pub reason: HookSkipReason,
}

pub trait HarnessAdapter: Sync {
    #[cfg(test)]
    fn name(&self) -> &str;

    fn plan(&self, bundle: &Bundle, project: &ProjectPaths) -> Vec<PlannedArtifact>;

    fn skips(&self, bundle: &Bundle) -> Vec<SkippedHook>;

    fn apply(&self, plan: &[PlannedArtifact], prior: &State) -> Result<Vec<AppliedArtifact>>;
}
