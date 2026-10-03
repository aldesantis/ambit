use std::io::Write as _;
use std::process::{Command, Stdio};

use super::*;

const TOKEN: &str = "gho_s3cr3tT0ken";

fn env_of(pairs: &[(&str, &str)]) -> Env {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

/// The `GIT_CONFIG_*` entries an environment carries, in order.
fn config_entries(env: &Env) -> Vec<(String, String)> {
    let count: usize = env[CONFIG_COUNT_VAR].parse().expect("a count");

    (0..count)
        .map(|index| {
            (
                env[&format!("GIT_CONFIG_KEY_{index}")].clone(),
                env[&format!("GIT_CONFIG_VALUE_{index}")].clone(),
            )
        })
        .collect()
}

#[test]
fn puts_the_token_in_one_variable_and_a_helper_that_reads_it() {
    let env = with_github_token(&env_of(&[("HOME", "/home/u")]), TOKEN);
    let entries = config_entries(&env);

    assert_eq!(env[GITHUB_TOKEN_VAR], TOKEN);
    assert_eq!(
        entries,
        [
            (
                "credential.https://github.com.helper".to_owned(),
                String::new()
            ),
            (
                "credential.https://github.com.helper".to_owned(),
                HELPER.to_owned()
            ),
            (
                "url.https://github.com/.insteadOf".to_owned(),
                "git@github.com:".to_owned()
            ),
            (
                "url.https://github.com/.insteadOf".to_owned(),
                "ssh://git@github.com/".to_owned()
            ),
        ]
    );

    let holding_token: Vec<&String> = env
        .iter()
        .filter(|(_, value)| value.contains(TOKEN))
        .map(|(name, _)| name)
        .collect();

    assert_eq!(holding_token, [GITHUB_TOKEN_VAR]);
}

#[test]
fn appends_after_config_entries_already_in_the_environment() {
    let base = env_of(&[
        ("GIT_CONFIG_COUNT", "1"),
        ("GIT_CONFIG_KEY_0", "core.autocrlf"),
        ("GIT_CONFIG_VALUE_0", "false"),
    ]);
    let env = with_github_token(&base, TOKEN);
    let entries = config_entries(&env);

    assert_eq!(entries.len(), 5);
    assert_eq!(entries[0], ("core.autocrlf".to_owned(), "false".to_owned()));
    assert_eq!(entries[1].0, "credential.https://github.com.helper");
}

#[test]
fn an_empty_token_changes_nothing() {
    let base = env_of(&[("HOME", "/home/u")]);

    assert_eq!(with_github_token(&base, ""), base);
}

/// Runs `git credential fill` for `url` with the token environment, as git itself would ask.
fn credential_fill(url: &str) -> String {
    let dir = crate::test_support::tempdir();
    let mut env = with_github_token(&crate::test_support::test_env(dir.path()), TOKEN);
    env.insert("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned());
    env.insert("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned());
    env.insert("GIT_ASKPASS".to_owned(), String::new());

    let mut child = Command::new("git")
        .args(["credential", "fill"])
        .current_dir(dir.path())
        .env_clear()
        .envs(&env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git runs");

    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(format!("url={url}\n\n").as_bytes())
        .expect("write the request");

    let output = child.wait_with_output().expect("git exits");

    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the helper runs through sh, which Windows CI may not put on PATH"
)]
fn git_receives_the_token_for_github_through_the_helper() {
    let filled = credential_fill("https://github.com/acme/private.git");

    assert!(filled.contains("username=x-access-token\n"), "{filled}");
    assert!(filled.contains(&format!("password={TOKEN}\n")), "{filled}");
}

#[test]
#[cfg_attr(
    windows,
    ignore = "the helper runs through sh, which Windows CI may not put on PATH"
)]
fn no_other_host_is_offered_the_token() {
    let filled = credential_fill("https://github.com.evil.test/acme/private.git");

    assert!(!filled.contains(TOKEN), "{filled}");
}

#[test]
fn redacts_the_token_and_url_credentials() {
    let env = with_github_token(&Env::new(), TOKEN);

    assert_eq!(
        redact(&format!("fatal: {TOKEN} rejected"), &env),
        "fatal: *** rejected"
    );
    assert_eq!(
        redact(
            "fatal: unable to access 'https://x-access-token:abc@github.com/acme/s.git/'",
            &env
        ),
        "fatal: unable to access 'https://***@github.com/acme/s.git/'"
    );
    assert_eq!(
        redact("cloning https://ghp_abc@github.com/acme/s", &Env::new()),
        "cloning https://***@github.com/acme/s"
    );
    assert_eq!(
        redact("ssh://git@github.com/acme/s.git", &Env::new()),
        "ssh://git@github.com/acme/s.git"
    );
    assert_eq!(
        redact("ssh://git:hunter2@example.test/s.git", &Env::new()),
        "ssh://git:***@example.test/s.git"
    );
    assert_eq!(
        redact("git@github.com:acme/s.git", &Env::new()),
        "git@github.com:acme/s.git"
    );
}

#[test]
fn classifies_what_git_and_github_say() {
    let cases: &[(&str, GitFailure)] = &[
        (
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            GitFailure::AuthRequired,
        ),
        (
            "remote: Invalid username or token. Password authentication is not supported for Git operations.\nfatal: Authentication failed for 'https://github.com/acme/s.git/'",
            GitFailure::AuthRequired,
        ),
        (
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
            GitFailure::AuthRequired,
        ),
        (
            "remote: Repository not found.\nfatal: repository 'https://github.com/acme/s.git/' not found",
            GitFailure::NotFound,
        ),
        (
            "fatal: '/srv/missing.git' does not appear to be a git repository\nfatal: Could not read from remote repository.",
            GitFailure::NotFound,
        ),
        (
            "remote: The 'acme' organization has enabled or enforced SAML SSO. To access this repository, you must re-authorize the OAuth Application.\nfatal: unable to access 'https://github.com/acme/s.git/': The requested URL returned error: 403",
            GitFailure::AccessDenied { sso: true },
        ),
        (
            "remote: Permission to acme/s.git denied to someone.\nfatal: unable to access 'https://github.com/acme/s.git/': The requested URL returned error: 403",
            GitFailure::AccessDenied { sso: false },
        ),
        (
            "remote: The `acme' organization has enabled OAuth App access restrictions.",
            GitFailure::AccessDenied { sso: false },
        ),
        (
            "fatal: couldn't find remote ref refs/heads/nope",
            GitFailure::RevisionNotFound,
        ),
        (
            "fatal: unable to access 'https://github.com/acme/s.git/': Could not resolve host: github.com",
            GitFailure::Network,
        ),
        (
            "ssh: connect to host github.com port 22: Connection refused",
            GitFailure::Network,
        ),
        ("fatal: something else entirely", GitFailure::Other),
        ("", GitFailure::Other),
    ];

    for (stderr, expected) in cases {
        assert_eq!(classify_git_failure(stderr), *expected, "{stderr}");
    }
}

#[test]
fn the_deciding_line_is_the_explanation_not_the_generic_fatal_line() {
    let stderr = "remote: The 'acme' organization has enabled or enforced SAML SSO.\nfatal: unable to access 'x': The requested URL returned error: 403\n";

    assert_eq!(
        deciding_line(stderr),
        Some((
            GitFailure::AccessDenied { sso: true },
            "remote: The 'acme' organization has enabled or enforced SAML SSO."
        ))
    );
}
