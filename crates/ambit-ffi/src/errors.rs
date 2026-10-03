//! The one error type every export throws, and its conversion from the core's [`AmbitError`].
//!
//! Every message and detail line passes through [`redact`] on the way out, so a token or a
//! credential embedded in a URL never reaches the app's UI or logs. Exports that can see the
//! engine's GitHub token convert through [`Engine::error`](crate::engine::Engine::error), which
//! also removes that literal value.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::LazyLock;

use ambit_core::errors::{AmbitError, ExitCode};
use ambit_core::util::fs::io_message;
use regex::Regex;

/// A failure, classified by what the app does about it.
#[derive(uniffi::Error, Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    /// The config, or something it names, is wrong. `path` and `line` locate it when known; `path`
    /// is the file as the message names it, relative to the setup root.
    Config {
        message: String,
        detail: Vec<String>,
        path: Option<String>,
        line: Option<u32>,
    },
    /// The selection cannot be resolved: a pattern matching nothing, a cycle, a name conflict.
    Resolution {
        message: String,
        detail: Vec<String>,
    },
    /// Fetching a catalog failed, or the cache cannot answer offline.
    Network {
        message: String,
        detail: Vec<String>,
        kind: NetworkKind,
    },
    /// A target path or config entry exists and ambit does not own it.
    OwnershipConflict {
        message: String,
        detail: Vec<String>,
        path: Option<String>,
    },
    /// Something the review depended on changed since it was made. Review again.
    StaleReview {
        message: String,
        detail: Vec<String>,
    },
    /// Another operation holds the setup's lock.
    Busy {
        message: String,
        detail: Vec<String>,
    },
    /// The caller canceled the operation.
    Canceled,
    /// A file system operation failed outside the config.
    Io {
        message: String,
        detail: Vec<String>,
        path: Option<String>,
    },
    /// A bug in ambit.
    Internal {
        message: String,
        detail: Vec<String>,
    },
}

/// Why a network operation failed, so the app can offer the right next step.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkKind {
    /// Offline, and the cache does not hold what was asked for.
    NotCached,
    /// The remote could not be reached.
    Offline,
    /// The remote wants credentials and none were given, or they were rejected.
    AuthRequired,
    /// The credentials were accepted but do not grant access. `sso` is set when an organization's
    /// SAML single sign-on has to authorize them first.
    AccessDenied {
        sso: bool,
    },
    /// The repository or ref does not exist, or is hidden from these credentials.
    NotFound,
    Other,
}

impl EngineError {
    /// The summary line, or a fixed one for [`EngineError::Canceled`].
    pub fn message(&self) -> &str {
        match self {
            Self::Config { message, .. }
            | Self::Resolution { message, .. }
            | Self::Network { message, .. }
            | Self::OwnershipConflict { message, .. }
            | Self::StaleReview { message, .. }
            | Self::Busy { message, .. }
            | Self::Io { message, .. }
            | Self::Internal { message, .. } => message,
            Self::Canceled => "the operation was canceled",
        }
    }

    /// Converts a core error, removing `secrets` and credential-shaped text from every line.
    ///
    /// Classification is by exit code. The core marks stale reviews, ownership conflicts and a held
    /// lock only through their messages, so the modules raising those convert with
    /// [`EngineError::stale_review`], [`EngineError::ownership_conflict`] and
    /// [`EngineError::busy`] at the call site instead.
    pub fn from_core(error: &AmbitError, secrets: &[&str]) -> Self {
        let message = redact(&error.message, secrets);
        let detail = redact_all(&error.detail, secrets);

        match error.code {
            ExitCode::Config => {
                let (path, line) = position(&message);

                Self::Config {
                    message,
                    detail,
                    path,
                    line,
                }
            }
            ExitCode::Resolution => Self::Resolution { message, detail },
            ExitCode::Network => Self::Network {
                kind: classify_network(&message, &detail),
                message,
                detail,
            },
            ExitCode::Canceled => Self::Canceled,
            // The app never asks for drift or doctor exit codes; reaching one is a bug.
            ExitCode::Success | ExitCode::Internal | ExitCode::Drift | ExitCode::Doctor => {
                Self::Internal { message, detail }
            }
        }
    }

    /// A core error reporting that the reviewed state changed, as [`EngineError::StaleReview`].
    pub fn stale_review(error: &AmbitError, secrets: &[&str]) -> Self {
        Self::StaleReview {
            message: redact(&error.message, secrets),
            detail: redact_all(&error.detail, secrets),
        }
    }

    /// A core error reporting an unowned target at `path`, as [`EngineError::OwnershipConflict`].
    pub fn ownership_conflict(error: &AmbitError, path: Option<&str>, secrets: &[&str]) -> Self {
        Self::OwnershipConflict {
            message: redact(&error.message, secrets),
            detail: redact_all(&error.detail, secrets),
            path: path.map(|path| redact(path, secrets)),
        }
    }

    /// A core error reporting that another operation holds the lock, as [`EngineError::Busy`].
    pub fn busy(error: &AmbitError, secrets: &[&str]) -> Self {
        Self::Busy {
            message: redact(&error.message, secrets),
            detail: redact_all(&error.detail, secrets),
        }
    }

    /// An I/O failure of `syscall` on `path`.
    pub fn io(error: &std::io::Error, syscall: &str, path: &Path) -> Self {
        Self::Io {
            message: format!("cannot {syscall} {}", path.display()),
            detail: vec![redact(&io_message(error, syscall, path), &[])],
            path: Some(path.display().to_string()),
        }
    }
}

impl From<AmbitError> for EngineError {
    fn from(error: AmbitError) -> Self {
        Self::from_core(&error, &[])
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for EngineError {}

/// Runs an export body, turning a panic into [`EngineError::Internal`] instead of unwinding into
/// Swift. Every fallible export wraps its body in this.
///
/// # Errors
///
/// Whatever `body` returns, or [`EngineError::Internal`] if it panicked.
pub fn guard<T>(body: impl FnOnce() -> Result<T, EngineError>) -> Result<T, EngineError> {
    // The body's state is discarded on panic: nothing it touched is observed afterwards except
    // through mutexes, whose poisoning every lock site tolerates.
    catch_unwind(AssertUnwindSafe(body)).unwrap_or_else(|payload| {
        let cause = payload
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_owned());

        Err(EngineError::Internal {
            message: "unexpected internal error".to_owned(),
            detail: vec![
                redact(&cause, &[]),
                "this is a bug in ambit; please report it".to_owned(),
            ],
        })
    })
}

const REDACTED: &str = "[redacted]";

/// Shorter secrets are not removed literally: replacing every occurrence of a two-letter string
/// would mangle the message without protecting anything.
const MIN_SECRET_LEN: usize = 8;

/// `scheme://user:password@`: the credential part of a URL.
static URL_USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*://)[^/@\s]+@").expect("a valid pattern")
});

/// GitHub's token formats: classic, OAuth, user-to-server, server-to-server, refresh, and
/// fine-grained.
static GITHUB_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b")
        .expect("a valid pattern")
});

/// An `Authorization` header value, as git prints it with tracing on.
static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(authorization:\s*(?:bearer|basic|token)\s+)\S+").expect("a valid pattern")
});

/// `text` with every credential-shaped substring and every literal secret replaced.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    let mut text = text.to_owned();

    for secret in secrets {
        if secret.len() >= MIN_SECRET_LEN {
            text = text.replace(secret, REDACTED);
        }
    }

    let text = URL_USERINFO.replace_all(&text, format!("${{1}}{REDACTED}@"));
    let text = GITHUB_TOKEN.replace_all(&text, REDACTED);
    let text = AUTH_HEADER.replace_all(&text, format!("${{1}}{REDACTED}"));

    text.into_owned()
}

fn redact_all(lines: &[String], secrets: &[&str]) -> Vec<String> {
    lines.iter().map(|line| redact(line, secrets)).collect()
}

/// The file and line from the `(file line N)` or `(file)` suffix that
/// [`ambit_core::errors::at`] renders at the end of a message.
fn position(message: &str) -> (Option<String>, Option<u32>) {
    let Some(inner) = message
        .strip_suffix(')')
        .and_then(|rest| rest.rsplit_once(" ("))
        .map(|(_, inner)| inner)
    else {
        return (None, None);
    };

    if let Some((file, line)) = inner.rsplit_once(" line ")
        && let Ok(line) = line.parse::<u32>()
    {
        return (Some(file.to_owned()), Some(line));
    }

    // A parenthesized aside that names no file is not a position.
    if inner.contains(' ') || !inner.contains('.') {
        return (None, None);
    }

    (Some(inner.to_owned()), None)
}

/// Sorts a network failure by the text git and the cache layer produce.
fn classify_network(message: &str, detail: &[String]) -> NetworkKind {
    let text = std::iter::once(message)
        .chain(detail.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));

    // Every error raised for a cache that cannot answer offline names the flag.
    if has(&["`--offline`"]) {
        return NetworkKind::NotCached;
    }

    if has(&["saml", "sso"]) {
        return NetworkKind::AccessDenied { sso: true };
    }

    if has(&[
        "authentication failed",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "invalid username or password",
    ]) {
        return NetworkKind::AuthRequired;
    }

    if has(&[
        "the requested url returned error: 403",
        "permission denied",
        "access denied",
        "permission to ",
    ]) {
        return NetworkKind::AccessDenied { sso: false };
    }

    if has(&[
        "repository not found",
        "not found",
        "does not appear to be a git repository",
        "couldn't find remote ref",
    ]) {
        return NetworkKind::NotFound;
    }

    if has(&[
        "could not resolve host",
        "failed to connect",
        "network is unreachable",
        "timed out",
        "connection refused",
        "unable to access",
    ]) {
        return NetworkKind::Offline;
    }

    NetworkKind::Other
}

#[cfg(test)]
mod tests {
    use ambit_core::errors::{at, config_error, network_error, resolution_error};
    use ambit_core::util::control::canceled;

    use super::*;

    #[test]
    fn config_errors_carry_the_file_and_line() {
        let error = config_error(
            format!("unknown key \"x\" {}", at("ambit.yml", Some(4))),
            ["remove it"],
        );

        assert_eq!(
            EngineError::from(error),
            EngineError::Config {
                message: "unknown key \"x\" (ambit.yml line 4)".to_owned(),
                detail: vec!["remove it".to_owned()],
                path: Some("ambit.yml".to_owned()),
                line: Some(4),
            }
        );
    }

    #[test]
    fn a_position_without_a_line_still_names_the_file() {
        assert_eq!(
            position("bad value (ambit.yaml)"),
            (Some("ambit.yaml".to_owned()), None)
        );
        assert_eq!(position("no ambit config in /tmp/x"), (None, None));
        assert_eq!(position("something (see the docs)"), (None, None));
    }

    #[test]
    fn maps_each_exit_code() {
        assert!(matches!(
            EngineError::from(resolution_error("cycle", ["a"])),
            EngineError::Resolution { .. }
        ));
        assert_eq!(EngineError::from(canceled()), EngineError::Canceled);
        assert!(matches!(
            EngineError::from(AmbitError::unexpected("boom")),
            EngineError::Internal { .. }
        ));
    }

    #[test]
    fn classifies_network_failures() {
        let kind = |message: &str, said: &str| match EngineError::from(network_error(
            message,
            [said.to_owned()],
        )) {
            EngineError::Network { kind, .. } => kind,
            other => panic!("not a network error: {other:?}"),
        };

        assert_eq!(
            kind(
                "catalog \"c\" is not in the cache (ambit.yml line 3)",
                "`--offline` was given"
            ),
            NetworkKind::NotCached
        );
        assert_eq!(
            kind(
                "cannot fetch",
                "git said: fatal: could not read Username for 'https://github.com': terminal prompts disabled"
            ),
            NetworkKind::AuthRequired
        );
        assert_eq!(
            kind(
                "cannot fetch",
                "git said: remote: The 'acme' organization has enabled or enforced SAML SSO."
            ),
            NetworkKind::AccessDenied { sso: true }
        );
        assert_eq!(
            kind(
                "cannot fetch",
                "git said: The requested URL returned error: 403"
            ),
            NetworkKind::AccessDenied { sso: false }
        );
        assert_eq!(
            kind("cannot fetch", "git said: remote: Repository not found."),
            NetworkKind::NotFound
        );
        assert_eq!(
            kind(
                "cannot fetch",
                "git said: Could not resolve host: github.com"
            ),
            NetworkKind::Offline
        );
        assert_eq!(
            kind("git is not on PATH", "install git"),
            NetworkKind::Other
        );
    }

    #[test]
    fn removes_credentials_from_every_line() {
        let token = "ghp_0123456789abcdefghijABCDEFGHIJ012345";
        let error = network_error(
            format!("cannot fetch https://x-access-token:{token}@github.com/acme/skills.git"),
            [
                format!("token {token}"),
                "Authorization: Bearer secretvalue".to_owned(),
                "custom s3cr3t-v4lue".to_owned(),
            ],
        );

        let converted = EngineError::from_core(&error, &["s3cr3t-v4lue"]);
        let EngineError::Network {
            message, detail, ..
        } = converted
        else {
            panic!("not a network error");
        };

        assert_eq!(
            message,
            "cannot fetch https://[redacted]@github.com/acme/skills.git"
        );
        assert_eq!(
            detail,
            [
                "token [redacted]",
                "Authorization: Bearer [redacted]",
                "custom [redacted]"
            ]
        );
    }

    #[test]
    fn short_secrets_are_left_alone() {
        assert_eq!(redact("an ab cd", &["ab"]), "an ab cd");
    }

    #[test]
    fn a_panic_becomes_an_internal_error() {
        let result: Result<(), EngineError> = guard(|| panic!("exploded"));

        assert_eq!(
            result,
            Err(EngineError::Internal {
                message: "unexpected internal error".to_owned(),
                detail: vec![
                    "exploded".to_owned(),
                    "this is a bug in ambit; please report it".to_owned()
                ],
            })
        );
    }

    #[test]
    fn io_errors_name_the_path() {
        let error = std::io::Error::from(std::io::ErrorKind::NotFound);
        let converted = EngineError::io(&error, "open", Path::new("/nowhere"));

        assert_eq!(
            converted,
            EngineError::Io {
                message: "cannot open /nowhere".to_owned(),
                detail: vec!["ENOENT: no such file or directory, open '/nowhere'".to_owned()],
                path: Some("/nowhere".to_owned()),
            }
        );
    }
}
