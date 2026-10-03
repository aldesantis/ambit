//! Plain data shared by every export: paths as strings, lines as `u32`, no behavior beyond
//! conversion from the core's types.
//!
//! Records used by more than one ffi module live here. A record only one module returns may live
//! in that module.

use ambit_core::model::config::ProjectConfig;
use ambit_core::model::git::is_commit_sha;
use ambit_core::model::pattern::PatternEntry;
use ambit_core::model::requirement;
use ambit_core::model::sources::{Source, SourceRequest, parse_source};
use ambit_core::util::control;

/// Which namespace a capability belongs to.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ItemKind {
    Skill,
    Pack,
    Mcp,
    Hook,
}

impl From<requirement::ItemKind> for ItemKind {
    fn from(kind: requirement::ItemKind) -> Self {
        match kind {
            requirement::ItemKind::Skill => Self::Skill,
            requirement::ItemKind::Pack => Self::Pack,
            requirement::ItemKind::Mcp => Self::Mcp,
            requirement::ItemKind::Hook => Self::Hook,
        }
    }
}

impl From<ItemKind> for requirement::ItemKind {
    fn from(kind: ItemKind) -> Self {
        match kind {
            ItemKind::Skill => Self::Skill,
            ItemKind::Pack => Self::Pack,
            ItemKind::Mcp => Self::Mcp,
            ItemKind::Hook => Self::Hook,
        }
    }
}

/// The step of a long operation a [`ProgressEvent`] belongs to.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stage {
    LoadingCatalogs,
    Fetching,
    Resolving,
    Planning,
    CheckingOwnership,
    SavingConfig,
    WritingFiles,
    RemovingFiles,
    WritingRecords,
}

impl From<control::Stage> for Stage {
    fn from(stage: control::Stage) -> Self {
        match stage {
            control::Stage::LoadingCatalogs => Self::LoadingCatalogs,
            control::Stage::Fetching => Self::Fetching,
            control::Stage::Resolving => Self::Resolving,
            control::Stage::Planning => Self::Planning,
            control::Stage::CheckingOwnership => Self::CheckingOwnership,
            control::Stage::SavingConfig => Self::SavingConfig,
            control::Stage::WritingFiles => Self::WritingFiles,
            control::Stage::RemovingFiles => Self::RemovingFiles,
            control::Stage::WritingRecords => Self::WritingRecords,
        }
    }
}

/// One progress report. `total` is 0 when the amount of work is not known in advance.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ProgressEvent {
    pub stage: Stage,
    /// What the step is working on: a catalog name, a path.
    pub subject: String,
    pub current: u32,
    pub total: u32,
}

impl From<&control::Progress> for ProgressEvent {
    fn from(progress: &control::Progress) -> Self {
        Self {
            stage: progress.stage.into(),
            subject: progress.subject.clone(),
            current: progress.current,
            total: progress.total,
        }
    }
}

/// What a setup's config looks like on disk right now.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SetupSnapshot {
    /// The setup root, as the session was opened with it.
    pub root: String,
    pub config: ConfigState,
}

/// The state of a setup's config file.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum ConfigState {
    /// No accepted config filename exists in the root.
    Missing,
    /// More than one accepted config filename exists. `files` lists them in preference order.
    Ambiguous {
        files: Vec<String>,
        problem: ConfigProblem,
    },
    /// The file exists but cannot be read, or does not parse or validate.
    Invalid {
        /// Absolute path.
        path: String,
        file_name: String,
        problem: ConfigProblem,
    },
    Valid {
        /// Absolute path.
        path: String,
        file_name: String,
        /// The file's exact contents, the base every edit starts from.
        text: String,
        summary: ConfigSummary,
    },
}

/// Why a config cannot be used, in the words the CLI would print.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ConfigProblem {
    pub message: String,
    /// The explanation and the next step, one line each.
    pub detail: Vec<String>,
    /// 1-based line the problem sits on, when the parser could position it.
    pub line: Option<u32>,
}

/// The parts of a valid config the app shows.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ConfigSummary {
    /// Agent tool ids, as `harnesses` lists them (or the default when absent).
    pub harnesses: Vec<String>,
    pub catalogs: Vec<CatalogEntry>,
    pub requires: Vec<SelectionEntry>,
}

impl From<&ProjectConfig> for ConfigSummary {
    fn from(config: &ProjectConfig) -> Self {
        Self {
            harnesses: config.harnesses.clone(),
            catalogs: config
                .catalogs
                .iter()
                .map(|catalog| CatalogEntry {
                    name: catalog.name.clone(),
                    source: catalog.source.clone(),
                    git_ref: catalog.r#ref.clone(),
                    source_kind: source_kind(&catalog.source, catalog.r#ref.as_deref()),
                })
                .collect(),
            requires: config.requires.iter().map(SelectionEntry::from).collect(),
        }
    }
}

/// One `catalogs` entry.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CatalogEntry {
    pub name: String,
    /// The `source` string as written.
    pub source: String,
    pub git_ref: Option<String>,
    /// Absent when `source` matches none of the accepted formats.
    pub source_kind: Option<SourceKind>,
}

/// What a `source` string names.
#[derive(uniffi::Enum, Clone, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A local directory. `path` is as written after `path:`, relative to the setup root.
    Local { path: String },
    /// A git repository. `url` is what git is asked for; `commit_ref` is whether the ref names a
    /// commit, which pins the catalog.
    Git {
        url: String,
        github: bool,
        commit_ref: bool,
    },
}

/// Reads `source` with the core's grammar, or `None` when it matches no format.
pub fn source_kind(source: &str, git_ref: Option<&str>) -> Option<SourceKind> {
    let request = SourceRequest {
        source: source.to_owned(),
        r#ref: git_ref.map(str::to_owned),
        ..SourceRequest::default()
    };

    parse_source(&request).ok().map(SourceKind::from)
}

impl From<Source> for SourceKind {
    fn from(source: Source) -> Self {
        match source {
            Source::Path { directory } => Self::Local { path: directory },
            Source::Git { url, r#ref } => Self::Git {
                github: is_github(&url),
                commit_ref: r#ref.as_deref().is_some_and(is_commit_sha),
                url,
            },
        }
    }
}

/// Whether git would reach `url` on github.com, over HTTPS or SSH.
fn is_github(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    let host = match lower.split_once("://") {
        Some((_, rest)) => rest.split('/').next().unwrap_or_default(),
        None => lower.split(':').next().unwrap_or_default(),
    };
    let host = host.rsplit('@').next().unwrap_or_default();
    let host = host.split(':').next().unwrap_or_default();

    host == "github.com" || host == "www.github.com"
}

/// One `requires` entry.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SelectionEntry {
    pub kind: ItemKind,
    /// The catalog the pattern is qualified with.
    pub catalog: Option<String>,
    pub pattern: String,
    /// Whether the pattern holds a wildcard, so it selects by rule rather than by name.
    pub is_rule: bool,
}

impl From<&PatternEntry> for SelectionEntry {
    fn from(entry: &PatternEntry) -> Self {
        Self {
            kind: entry.kind.into(),
            catalog: entry.catalog.clone(),
            pattern: entry.pattern.clone(),
            is_rule: entry.pattern.contains('*'),
        }
    }
}

/// What a source the user typed names, and the catalog name to suggest for it.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub kind: SourceKind,
    pub proposed_name: String,
}

/// One supported agent tool and what it cannot do.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct AgentToolInfo {
    /// The id `harnesses` uses.
    pub id: String,
    pub display_name: String,
    /// Where the tool reads skills from, relative to the setup root.
    pub skills_dir: String,
    /// The MCP config file in a project setup, relative to the project root.
    pub mcp_file: String,
    /// The MCP config file in the Personal setup, relative to the home directory.
    pub personal_mcp_file: String,
    /// The hooks file, or absent for a tool with no declarative hooks.
    pub hooks_file: Option<String>,
    /// Plain-language limitations to show before Apply and in Health.
    pub limitations: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_each_source_format() {
        assert_eq!(
            source_kind("path:../catalog", None),
            Some(SourceKind::Local {
                path: "../catalog".to_owned()
            })
        );
        assert_eq!(
            source_kind("acme/skills", Some(&"a".repeat(40))),
            Some(SourceKind::Git {
                url: "https://github.com/acme/skills.git".to_owned(),
                github: true,
                commit_ref: true,
            })
        );
        assert_eq!(
            source_kind("git@github.com:acme/skills.git", Some("main")),
            Some(SourceKind::Git {
                url: "git@github.com:acme/skills.git".to_owned(),
                github: true,
                commit_ref: false,
            })
        );
        assert_eq!(
            source_kind("https://gitlab.com/acme/skills.git", None),
            Some(SourceKind::Git {
                url: "https://gitlab.com/acme/skills.git".to_owned(),
                github: false,
                commit_ref: false,
            })
        );
        assert_eq!(source_kind("not a source", None), None);
    }

    #[test]
    fn a_wildcard_makes_a_rule() {
        let entry = PatternEntry {
            kind: requirement::ItemKind::Skill,
            pattern: "core.*".to_owned(),
            catalog: Some("company".to_owned()),
        };

        assert_eq!(
            SelectionEntry::from(&entry),
            SelectionEntry {
                kind: ItemKind::Skill,
                catalog: Some("company".to_owned()),
                pattern: "core.*".to_owned(),
                is_rule: true,
            }
        );
    }
}
