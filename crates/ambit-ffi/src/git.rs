//! The environment git runs in for the app, and a check that the configured git works.
//!
//! [`Engine::env`] is the app's environment without credentials. Every module that reaches git
//! builds its `Env` through [`git_env`] instead, which adds the signed-in GitHub token
//! ([`with_github_token`]) and the settings that keep git from ever waiting on a prompt: a
//! prompt the app cannot show is indistinguishable from a hang, so git fails visibly instead.

use std::path::Path;

use ambit_core::model::git::credentials::with_github_token;
use ambit_core::model::git::run_git;
use ambit_core::util::env::Env;

use crate::engine::Engine;
use crate::errors::{EngineError, guard};

/// Settings set unless the app's environment already chose a value. `GIT_TERMINAL_PROMPT=0` is
/// always set by core.
const PROMPT_FREE: &[(&str, &str)] = &[
    // An askpass program that answers nothing, so git asks no GUI helper for a password.
    ("GIT_ASKPASS", "/usr/bin/false"),
    ("SSH_ASKPASS", "/usr/bin/false"),
    ("SSH_ASKPASS_REQUIRE", "never"),
    // ssh fails instead of asking for a passphrase or to trust a new host key.
    ("GIT_SSH_COMMAND", "ssh -o BatchMode=yes"),
    // Git Credential Manager, when installed, otherwise opens a browser window.
    ("GCM_INTERACTIVE", "never"),
];

/// The environment for an operation that runs git: the app's, plus the GitHub token when signed
/// in, plus [`PROMPT_FREE`] defaults.
pub fn git_env(engine: &Engine) -> Env {
    with_git_settings(engine.env(), engine.github_token().as_deref())
}

fn with_git_settings(base: &Env, token: Option<&str>) -> Env {
    let mut env = match token {
        Some(token) => with_github_token(base, token),
        None => base.clone(),
    };

    for (name, value) in PROMPT_FREE {
        env.entry((*name).to_owned())
            .or_insert_with(|| (*value).to_owned());
    }

    env
}

#[uniffi::export]
impl Engine {
    /// What `git --version` prints for the git the engine is configured with
    /// (`AMBIT_GIT_PROGRAM`, else `git` on the configured `PATH`), to confirm it runs.
    ///
    /// # Errors
    ///
    /// [`EngineError::Network`] when git cannot be started or exits with an error.
    pub fn git_version(&self) -> Result<String, EngineError> {
        guard(|| {
            let env = git_env(self);
            let cwd = env.get("HOME").map_or_else(|| Path::new("/"), Path::new);
            let outcome = run_git(&["--version"], cwd, &env).map_err(|error| self.error(&error))?;

            if !outcome.ok {
                return Err(self.error(&ambit_core::errors::network_error(
                    "git --version failed",
                    [outcome.stderr.trim().to_owned()],
                )));
            }

            Ok(outcome.stdout.trim().to_owned())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use ambit_core::model::git::GIT_PROGRAM_VAR;
    use ambit_core::model::git::credentials::GITHUB_TOKEN_VAR;

    use super::*;
    use crate::engine::EngineConfig;

    fn engine(env: &[(&str, &str)]) -> std::sync::Arc<Engine> {
        Engine::new(EngineConfig {
            env: env
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect::<HashMap<_, _>>(),
        })
    }

    #[test]
    fn adds_the_token_only_while_signed_in() {
        let engine = engine(&[("HOME", "/home/u")]);

        assert!(!git_env(&engine).contains_key(GITHUB_TOKEN_VAR));
        assert!(!engine.env().contains_key(GITHUB_TOKEN_VAR));

        engine.set_github_token(Some("gho_abcdefgh".to_owned()));

        assert_eq!(git_env(&engine)[GITHUB_TOKEN_VAR], "gho_abcdefgh");
        assert!(!engine.env().contains_key(GITHUB_TOKEN_VAR));

        engine.set_github_token(None);

        assert!(!git_env(&engine).contains_key(GITHUB_TOKEN_VAR));
    }

    #[test]
    fn never_lets_git_prompt_but_keeps_the_apps_own_choices() {
        let env = with_git_settings(
            &[("GIT_SSH_COMMAND".to_owned(), "ssh -i key".to_owned())].into(),
            None,
        );

        assert_eq!(env["GIT_ASKPASS"], "/usr/bin/false");
        assert_eq!(env["GCM_INTERACTIVE"], "never");
        assert_eq!(env["GIT_SSH_COMMAND"], "ssh -i key");
    }

    #[test]
    fn reports_a_configured_git_that_is_not_there() {
        let dir = tempfile::tempdir().expect("a tempdir");
        let missing = dir.path().join("no-git");
        let missing = missing.to_string_lossy();
        let engine = engine(&[(GIT_PROGRAM_VAR, &missing)]);

        let error = engine.git_version().expect_err("no git there");

        assert!(
            matches!(&error, EngineError::Network { message, .. } if message.contains(&*missing)),
            "{error:?}"
        );
    }
}
