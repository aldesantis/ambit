#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::model::catalog::{CatalogParseOptions, SKILL_FILENAME, parse_catalog_directory};
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::tempdir;

const DOTAGENTS_PACKAGE: &str = "@sentry/dotagents";

const SKIP_VAR: &str = "AMBIT_SKIP_NETWORK_TESTS";

/// dotagents refuses a `path:` source resolving outside the project root.
const CATALOG_DIRNAME: &str = "catalog";

const AGENTS_DIRNAME: &str = ".agents";
const INSTALLED_DIR: &str = ".agents/skills";
const CLAUDE_LINK: &str = ".claude/skills";

/// npm's retry-with-backoff turns no network into a minute of silence; retry only in CI.
const IMPATIENT_NPM: &[(&str, &str)] = &[
    ("npm_config_fetch_retries", "0"),
    ("npm_config_fetch_timeout", "20000"),
];

const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

const QUOTED_STDERR_LINES: usize = 3;

fn is_set(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.is_empty())
}

struct ChildResult {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// dotagents defaults its cache and user-level directories under `$HOME`, so they are
/// redirected into `home`.
fn dotagents(args: &[&str], cwd: &Path, home: &Path) -> ChildResult {
    let mut command = Command::new("npx");

    command
        .arg("--yes")
        .arg(DOTAGENTS_PACKAGE)
        .args(args)
        .current_dir(cwd)
        .env("DOTAGENTS_HOME", home)
        .env("DOTAGENTS_STATE_DIR", home.join("state"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if !is_set("CI") {
        command.envs(IMPATIENT_NPM.iter().copied());
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ChildResult {
                code: None,
                stdout: String::new(),
                stderr: error.to_string(),
            };
        }
    };

    let mut stdout_pipe = child.stdout.take().expect("piped stdout");
    let mut stderr_pipe = child.stderr.take().expect("piped stderr");
    let stdout_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout_pipe.read_to_string(&mut text);
        text
    });
    let stderr_reader = thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr_pipe.read_to_string(&mut text);
        text
    });

    let (sender, receiver) = mpsc::channel();
    let pid_child = std::sync::Arc::new(std::sync::Mutex::new(child));
    let waiter = std::sync::Arc::clone(&pid_child);

    thread::spawn(move || {
        loop {
            let status = waiter.lock().expect("child lock").try_wait();

            match status {
                Ok(Some(status)) => {
                    let _ = sender.send(status.code());
                    return;
                }
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                Err(_) => {
                    let _ = sender.send(None);
                    return;
                }
            }
        }
    });

    let code = receiver.recv_timeout(CHILD_TIMEOUT).unwrap_or_else(|_| {
        let _ = pid_child.lock().expect("child lock").kill();
        None
    });

    ChildResult {
        code,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    }
}

fn write_dotagents_project(dir: &Path) {
    fs::create_dir_all(dir).expect("create the project");
    fs::write(
        dir.join("agents.toml"),
        format!(
            "version = 1\nagents = [\"claude\"]\n\n[[skills]]\nname = \"*\"\nsource = \"path:./{CATALOG_DIRNAME}\"\n"
        ),
    )
    .expect("write agents.toml");
    fs::write(
        dir.join(".gitignore"),
        format!("agents.lock\n{AGENTS_DIRNAME}/.gitignore\n"),
    )
    .expect("write .gitignore");
}

fn first_lines(text: &str) -> String {
    text.split('\n')
        .filter(|line| !line.trim().is_empty())
        .take(QUOTED_STDERR_LINES)
        .collect::<Vec<_>>()
        .join("\n")
}

fn probe(home: &Path) -> Option<String> {
    if is_set(SKIP_VAR) {
        return Some(format!("{SKIP_VAR} is set"));
    }

    let result = dotagents(&["--version"], &std::env::temp_dir(), home);

    if result.code == Some(0) {
        return None;
    }

    let code = result
        .code
        .map_or_else(|| "null".to_owned(), |code| code.to_string());

    Some(format!(
        "cannot run `npx {DOTAGENTS_PACKAGE}` (exit {code}), so the compatibility promise is \
         unverified. This is the one test that needs network access; set {SKIP_VAR}=1 to skip it \
         deliberately.\n{}",
        first_lines(&result.stderr)
    ))
}

#[test]
#[cfg_attr(
    windows,
    ignore = "dotagents refuses every `path:` source on Windows: it checks containment with a hard-coded `/`"
)]
fn installs_every_skill_in_the_hand_written_fixture_catalog_ignoring_ambits_additions() {
    let home = tempdir();

    if let Some(reason) = probe(home.path()) {
        assert!(!is_set("CI"), "{reason}");
        eprintln!("skipping the dotagents compatibility test: {reason}");
        return;
    }

    let root = tempdir();
    let project = root.path().join("project");
    let catalog_dir = project.join(CATALOG_DIRNAME);

    write_dotagents_project(&project);
    build_fixture_catalog(&catalog_dir).expect("build the fixture catalog");

    let catalog = parse_catalog_directory(
        "subject",
        &format!("path:{}", catalog_dir.display()),
        &catalog_dir,
        None,
        &mut CatalogParseOptions::default(),
    )
    .expect("ambit parses the fixture");

    assert_ne!(catalog.skills.len(), 0);

    // dotagents defaults to the global scope: a bare `install` writes `~/.agents/agents.toml`.
    let result = dotagents(&["--project", "install"], &project, home.path());

    assert_eq!(result.code, Some(0), "{}\n{}", result.stdout, result.stderr);

    let mut installed: Vec<String> = fs::read_dir(project.join(INSTALLED_DIR))
        .expect("list the installed skills")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    installed.sort();

    let mut expected: Vec<String> = catalog
        .skills
        .iter()
        .map(|skill| skill.name.clone())
        .collect();
    expected.sort();
    assert_eq!(installed, expected);

    let mut mismatched: BTreeMap<String, (String, String)> = BTreeMap::new();

    for skill in &catalog.skills {
        let source = fs::read_to_string(catalog_dir.join(&skill.path).join(SKILL_FILENAME))
            .expect("read the catalog's SKILL.md");
        let target = fs::read_to_string(
            project
                .join(INSTALLED_DIR)
                .join(&skill.name)
                .join(SKILL_FILENAME),
        )
        .expect("read the installed SKILL.md");

        if source != target {
            mismatched.insert(skill.name.clone(), (source, target));
        }
    }

    assert!(mismatched.is_empty(), "SKILL.md changed: {mismatched:?}");

    assert!(
        fs::metadata(project.join(CLAUDE_LINK)).is_ok(),
        "{CLAUDE_LINK} is missing"
    );
}
