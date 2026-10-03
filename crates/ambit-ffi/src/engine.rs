//! The engine handle, setup sessions, and the exports that need neither.
//!
//! Every export is a blocking call: the app runs them off the main thread. Nothing here reads
//! process state; the environment comes from [`EngineConfig`].

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ambit_core::errors::AmbitError;
use ambit_core::harness::definitions::PROFILES;
use ambit_core::harness::profile::{HarnessProfile, SHARED_SKILLS_DIR};
use ambit_core::model::config::{existing_config_files, find_config_file, load_project_config};
use ambit_core::model::sources::{Source, SourceRequest, parse_source};
use ambit_core::util::env::Env;
use ambit_core::util::fs::{canonicalize, io_message, read_text};

use crate::errors::{EngineError, guard, redact};
use crate::records::{
    AgentToolInfo, ConfigProblem, ConfigState, ConfigSummary, SetupSnapshot, SourceInfo,
};

/// What the app hands the engine at construction.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct EngineConfig {
    /// The complete environment every operation sees, built by the app: `HOME`, `PATH`,
    /// `XDG_CACHE_HOME`, `AMBIT_GIT_PROGRAM` and so on. The process environment is never read.
    pub env: HashMap<String, String>,
}

/// The engine handle the app holds for its lifetime.
#[derive(uniffi::Object)]
pub struct Engine {
    env: Env,
    /// Held in memory only, never written anywhere.
    token: Mutex<Option<String>>,
}

#[uniffi::export]
impl Engine {
    #[uniffi::constructor]
    pub fn new(config: EngineConfig) -> Arc<Self> {
        Arc::new(Self {
            env: config.env.into_iter().collect(),
            token: Mutex::new(None),
        })
    }

    /// Replaces the GitHub token later operations authenticate git with. `None` signs out.
    pub fn set_github_token(&self, token: Option<String>) {
        *lock(&self.token) = token.filter(|token| !token.is_empty());
    }

    pub fn has_github_token(&self) -> bool {
        lock(&self.token).is_some()
    }

    /// A session on the setup rooted at `root`. Touches no files.
    pub fn open_setup(self: Arc<Self>, root: String) -> Arc<SetupSession> {
        Arc::new(SetupSession {
            root: PathBuf::from(root),
            engine: self,
            state: Mutex::new(HashMap::new()),
        })
    }

    /// `path` with symlinks resolved, for deduplicating setups that name one folder two ways.
    ///
    /// # Errors
    ///
    /// [`EngineError::Io`] when the path does not exist or cannot be resolved.
    pub fn canonical_path(&self, path: &str) -> Result<String, EngineError> {
        guard(|| {
            let path = Path::new(path);

            canonicalize(path)
                .map(|resolved| resolved.display().to_string())
                .map_err(|error| EngineError::io(&error, "resolve", path))
        })
    }
}

impl Engine {
    /// The environment the app supplied, without credentials. Modules that run git add the token
    /// themselves from [`Engine::github_token`].
    pub fn env(&self) -> &Env {
        &self.env
    }

    pub fn github_token(&self) -> Option<String> {
        lock(&self.token).clone()
    }

    /// Converts a core error, removing the current GitHub token along with every credential-shaped
    /// substring. Prefer this to `EngineError::from` wherever an engine is at hand.
    pub fn error(&self, error: &AmbitError) -> EngineError {
        let token = self.github_token();

        EngineError::from_core(error, &token.as_deref().into_iter().collect::<Vec<_>>())
    }
}

/// Per-module state a session carries, keyed by type, so each ffi module can keep its own (loaded
/// catalogs, the last review) without this file naming it.
type SessionState = HashMap<TypeId, Box<dyn Any + Send>>;

/// One setup (the Personal setup or a project) the app has open.
#[derive(uniffi::Object)]
pub struct SetupSession {
    root: PathBuf,
    engine: Arc<Engine>,
    state: Mutex<SessionState>,
}

#[uniffi::export]
impl SetupSession {
    /// The root as the session was opened with it.
    pub fn root(&self) -> String {
        self.root.display().to_string()
    }

    /// Reads the config file and reports its state. Never fetches and never writes.
    ///
    /// # Errors
    ///
    /// Only [`EngineError::Internal`]: every problem with the config is a [`ConfigState`].
    pub fn snapshot(&self) -> Result<SetupSnapshot, EngineError> {
        guard(|| {
            Ok(SetupSnapshot {
                root: self.root(),
                config: config_state(&self.root, &self.engine),
            })
        })
    }
}

impl SetupSession {
    pub fn root_path(&self) -> &Path {
        &self.root
    }

    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Runs `body` on this session's `T`, created with `T::default()` on first use.
    ///
    /// The session's state is locked for the duration, so `body` must not block on I/O or call
    /// back into `with_state`: copy what a long operation needs out, and store the result after.
    ///
    /// # Panics
    ///
    /// Never: a slot is created for, and only ever holds, the type it is keyed by.
    pub fn with_state<T: Default + Send + 'static, R>(&self, body: impl FnOnce(&mut T) -> R) -> R {
        let mut state = lock(&self.state);
        let slot = state
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(T::default()));

        body(
            slot.downcast_mut::<T>()
                .expect("a slot always holds the type it is keyed by"),
        )
    }
}

/// Locks `mutex`, tolerating poisoning: [`guard`] contains panics, and every value behind these
/// locks is replaced whole, so a poisoned one is still consistent.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn config_state(root: &Path, engine: &Engine) -> ConfigState {
    let problem = |error: AmbitError| match engine.error(&error) {
        EngineError::Config {
            message,
            detail,
            line,
            ..
        } => ConfigProblem {
            message,
            detail,
            line,
        },
        other => ConfigProblem {
            message: other.message().to_owned(),
            detail: Vec::new(),
            line: None,
        },
    };

    // Never fails in practice; see `existing_config_files`.
    let files = existing_config_files(root).unwrap_or_default();

    if files.is_empty() {
        return ConfigState::Missing;
    }

    let found = match find_config_file(root) {
        Ok(found) => found,
        Err(error) if files.len() > 1 => {
            return ConfigState::Ambiguous {
                files,
                problem: problem(error),
            };
        }
        // The file vanished between the two checks.
        Err(_) => return ConfigState::Missing,
    };
    let path = found.path.display().to_string();

    // The parser reads the file itself, so the text is read a second time. A concurrent edit
    // between the two is caught by the app's external-change check, which compares this text.
    let config = match load_project_config(root) {
        Ok(config) => config,
        Err(error) => {
            return ConfigState::Invalid {
                path,
                file_name: found.file,
                problem: problem(error),
            };
        }
    };

    match read_text(&found.path) {
        Ok(text) => ConfigState::Valid {
            path,
            file_name: found.file,
            text,
            summary: ConfigSummary::from(&config),
        },
        Err(error) => ConfigState::Invalid {
            problem: ConfigProblem {
                message: format!("cannot read {}", found.file),
                detail: vec![redact(&io_message(&error, "open", &found.path), &[])],
                line: None,
            },
            path,
            file_name: found.file,
        },
    }
}

/// What `source` names and the catalog name to suggest for it: the repository or folder name.
///
/// # Errors
///
/// [`EngineError::Config`] when `source` matches none of the accepted formats, or its `@ref`
/// contradicts `git_ref`.
#[uniffi::export]
pub fn describe_source(source: String, git_ref: Option<String>) -> Result<SourceInfo, EngineError> {
    guard(|| {
        let request = SourceRequest {
            source,
            r#ref: git_ref,
            subject: "the source".to_owned(),
            r#where: String::new(),
            ..SourceRequest::default()
        };
        let parsed = parse_source(&request).map_err(|error| {
            let mut converted = EngineError::from(error);

            // `where` is empty here, so the message ends in a stray space.
            if let EngineError::Config { message, .. } = &mut converted {
                *message = message.trim_end().to_owned();
            }

            converted
        })?;
        let proposed_name = proposed_name(match &parsed {
            Source::Path { directory } => directory,
            Source::Git { url, .. } => url,
        });

        Ok(SourceInfo {
            kind: parsed.into(),
            proposed_name,
        })
    })
}

/// The last path segment of a directory or URL, without `.git`, in lowercase.
fn proposed_name(location: &str) -> String {
    let trimmed = location.trim_end_matches(['/', '\\']);
    let segment = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or_default();
    let segment = segment.strip_suffix(".git").unwrap_or(segment);

    if segment.is_empty() || segment == "." || segment == ".." {
        return "catalog".to_owned();
    }

    // A catalog alias cannot hold the address separator; nothing else is restricted.
    segment.to_lowercase().replace('/', "-")
}

/// The five agent tools ambit installs for, in the order the CLI lists them, with what each one
/// cannot do.
#[uniffi::export]
pub fn supported_agent_tools() -> Vec<AgentToolInfo> {
    PROFILES.iter().map(|profile| agent_tool(profile)).collect()
}

fn agent_tool(profile: &HarnessProfile) -> AgentToolInfo {
    let (display_name, limitations): (&str, &[&str]) = match profile.name {
        "claude" => ("Claude Code", &[]),
        "codex" => (
            "Codex",
            &["In a project, hook scripts are found only when Codex runs from the project folder."],
        ),
        "cursor" => (
            "Cursor",
            &[
                "In a project, hook scripts are found only when Cursor runs from the project folder.",
            ],
        ),
        "opencode" => (
            "OpenCode",
            &["Hooks are not supported. Hooks you select are skipped for OpenCode."],
        ),
        "vscode" => (
            "VS Code",
            &[
                "Hooks are written to Claude Code's settings file, which VS Code also reads.",
                "VS Code may not run hooks that ship a script in a project, because it does not document the project folder placeholder those hooks use.",
            ],
        ),
        other => (other, &[]),
    };

    AgentToolInfo {
        id: profile.name.to_owned(),
        display_name: display_name.to_owned(),
        skills_dir: profile.skills_link.unwrap_or(SHARED_SKILLS_DIR).to_owned(),
        mcp_file: profile.mcp.file.to_owned(),
        personal_mcp_file: profile.mcp.user_file.unwrap_or(profile.mcp.file).to_owned(),
        hooks_file: profile.hooks.as_ref().map(|hooks| hooks.file.to_owned()),
        limitations: limitations.iter().map(|&line| line.to_owned()).collect(),
    }
}

#[cfg(test)]
mod tests;
