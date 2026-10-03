//! GitHub credentials for git, redaction of secrets in what git says, and classification of why a
//! git command failed.
//!
//! The token reaches git only through the child's environment. [`with_github_token`] stores it in
//! [`GITHUB_TOKEN_VAR`] and adds `GIT_CONFIG_*` entries that install a credential helper for
//! `https://github.com`; the helper is a shell function that prints the variable, so the token is
//! never in an argument list, a URL, a remote's config, the cache key, or a log line. Git applies
//! URL-scoped `credential.<url>.*` keys by exact host, so no other host is ever offered the token.

use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::util::env::Env;

/// The variable holding the GitHub token in git's environment.
pub const GITHUB_TOKEN_VAR: &str = "AMBIT_GITHUB_TOKEN";

/// What a redacted secret is replaced with.
pub const REDACTED: &str = "***";

const CONFIG_COUNT_VAR: &str = "GIT_CONFIG_COUNT";

/// The URL prefix the helper and the rewrites are scoped to.
const GITHUB_URL: &str = "https://github.com";

/// Prints the token in git's credential protocol, for `get` requests only.
///
/// `x-access-token` is the username GitHub accepts alongside an OAuth or app token.
const HELPER: &str = "!f() { test \"$1\" = get && printf 'username=x-access-token\\npassword=%s\\n' \"$AMBIT_GITHUB_TOKEN\"; }; f";

/// The SSH spellings of a GitHub remote that are fetched over HTTPS while a token is present, so
/// a source written as `git@github.com:acme/skills.git` can use the same credentials. The saved
/// source and the clone's remote config keep the spelling the project wrote.
const SSH_PREFIXES: &[&str] = &["git@github.com:", "ssh://git@github.com/"];

/// `env` with `token` available to git for `https://github.com`.
///
/// Appends to any `GIT_CONFIG_COUNT` entries already present. The helper list for GitHub is reset
/// first, so a credential manager configured on the machine cannot answer instead of the token or
/// prompt. An empty token returns `env` unchanged.
pub fn with_github_token(env: &Env, token: &str) -> Env {
    let mut copy = env.clone();

    if token.is_empty() {
        return copy;
    }

    let helper_key = format!("credential.{GITHUB_URL}.helper");
    let rewrite_key = format!("url.{GITHUB_URL}/.insteadOf");
    let mut entries = vec![
        (helper_key.clone(), String::new()),
        (helper_key, HELPER.to_owned()),
    ];
    entries.extend(
        SSH_PREFIXES
            .iter()
            .map(|prefix| (rewrite_key.clone(), (*prefix).to_owned())),
    );

    // An unreadable count is one git would refuse anyway; starting over gives it a valid one.
    let start: usize = copy
        .get(CONFIG_COUNT_VAR)
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0);

    for (offset, (key, value)) in entries.iter().enumerate() {
        copy.insert(format!("GIT_CONFIG_KEY_{}", start + offset), key.clone());
        copy.insert(
            format!("GIT_CONFIG_VALUE_{}", start + offset),
            value.clone(),
        );
    }

    copy.insert(
        CONFIG_COUNT_VAR.to_owned(),
        (start + entries.len()).to_string(),
    );
    copy.insert(GITHUB_TOKEN_VAR.to_owned(), token.to_owned());
    copy
}

/// `scheme://userinfo@`, where a URL carries credentials.
static USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b([a-z][a-z0-9+.-]*)://([^/@\s]+)@").expect("a valid pattern")
});

/// `text` with the GitHub token in `env` and every URL's credentials replaced by [`REDACTED`].
///
/// An SSH URL keeps its user name, which is an account name like `git` rather than a secret, and
/// loses only a password. Every other scheme loses the whole user info, since a token is often
/// written as the user name alone (`https://<token>@github.com/...`).
pub fn redact(text: &str, env: &Env) -> String {
    let mut redacted = text.to_owned();

    if let Some(token) = env.get(GITHUB_TOKEN_VAR)
        && !token.is_empty()
    {
        redacted = redacted.replace(token.as_str(), REDACTED);
    }

    USERINFO
        .replace_all(&redacted, |captures: &Captures<'_>| {
            let scheme = &captures[1];
            let userinfo = &captures[2];
            let is_ssh = scheme.to_ascii_lowercase().contains("ssh");

            let kept = match userinfo.split_once(':') {
                Some((user, _)) if is_ssh => format!("{user}:{REDACTED}"),
                None if is_ssh => userinfo.to_owned(),
                _ => REDACTED.to_owned(),
            };

            format!("{scheme}://{kept}@")
        })
        .into_owned()
}

/// Why a git command that reached for a remote failed, as far as its output says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GitFailure {
    /// The remote wants credentials that were not given, or rejected the ones that were.
    AuthRequired,
    /// The credentials were accepted but do not grant access. `sso` when an organization's SAML
    /// single sign-on has not authorized them.
    AccessDenied {
        sso: bool,
    },
    /// No repository there, or none these credentials may see: GitHub answers both alike.
    NotFound,
    /// The repository exists but has no such branch, tag, or commit.
    RevisionNotFound,
    /// The remote could not be reached.
    Network,
    Other,
}

type Rule = (fn(&str) -> bool, GitFailure);

/// Checked in order against every line, lowercased; the first rule any line matches wins. More
/// specific causes come first, since git often follows them with a generic line.
const RULES: &[Rule] = &[
    (
        |line| line.contains("saml") || line.contains("single sign-on"),
        GitFailure::AccessDenied { sso: true },
    ),
    (
        |line| {
            line.contains("oauth app access restrictions")
                || line.contains("denied to ")
                || line.contains("error: 403")
                || line.contains("403 forbidden")
        },
        GitFailure::AccessDenied { sso: false },
    ),
    (
        |line| {
            line.contains("could not read username")
                || line.contains("could not read password")
                || line.contains("authentication failed")
                || line.contains("invalid username or")
                || line.contains("terminal prompts disabled")
                || line.contains("unable to read askpass")
                || line.contains("permission denied (publickey")
                || line.contains("error: 401")
                || line.contains("password authentication is not supported")
        },
        GitFailure::AuthRequired,
    ),
    (
        |line| {
            line.contains("repository not found")
                || line.contains("does not appear to be a git repository")
                || line.contains("error: 404")
                || (line.contains("repository '") && line.contains("' not found"))
        },
        GitFailure::NotFound,
    ),
    (
        |line| {
            line.contains("couldn't find remote ref")
                || line.contains("unknown revision")
                || line.contains("not a valid object name")
                || line.contains("invalid reference")
                || (line.contains("remote branch") && line.contains("not found"))
        },
        GitFailure::RevisionNotFound,
    ),
    (
        |line| {
            line.contains("could not resolve host")
                || line.contains("failed to connect")
                || line.contains("connection timed out")
                || line.contains("operation timed out")
                || line.contains("connection refused")
                || line.contains("connection reset")
                || line.contains("network is unreachable")
                || line.contains("ssl certificate")
                || line.contains("ssl connect")
                || line.contains("gnutls")
                || line.contains("tls handshake")
                || line.contains("unable to access")
                || line.contains("remote end hung up unexpectedly")
                || line.contains("early eof")
        },
        GitFailure::Network,
    ),
];

/// Classifies a failed git command by its standard error.
pub fn classify_git_failure(stderr: &str) -> GitFailure {
    deciding_line(stderr).map_or(GitFailure::Other, |(failure, _)| failure)
}

/// The classification, and the line of `stderr` that decided it.
///
/// An error quotes this line rather than git's last one: GitHub explains an SSO or access failure
/// in a `remote:` line that a generic `fatal:` line follows.
pub(crate) fn deciding_line(stderr: &str) -> Option<(GitFailure, &str)> {
    let lines: Vec<(&str, String)> = stderr
        .split(['\n', '\r'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| (line, line.to_lowercase()))
        .collect();

    RULES.iter().find_map(|(matches, failure)| {
        lines
            .iter()
            .find(|(_, lower)| matches(lower))
            .map(|(line, _)| (*failure, *line))
    })
}

#[cfg(test)]
mod tests;
