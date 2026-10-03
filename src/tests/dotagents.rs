//! The dotagents compatibility promise, made executable.
//!
//! ambit replaces dotagents, but a catalog must stay a plain skills repo so that dotagents (or
//! skills.sh, or anything else that reads `skills/<name>/SKILL.md`) can install from the same
//! directory. ambit's additions (`mcps/`, `hooks/`, the extra frontmatter keys) are supposed to be
//! additive and ignored. That is the guarantee most likely to rot, which is why it is checked by
//! running the real tool instead of by reasoning about it.
//!
//! The claim is asserted against ambit's own answer rather than a hand-written list:
//! [`parse_catalog_directory`] says which skills the catalog holds, and dotagents must install
//! exactly that set, under exactly the names ambit derives from the paths, with each `SKILL.md`
//! byte-identical to the source. So a frontmatter key that made another parser choke, an `mcps/`
//! entity mistaken for a skill, or a nested skill directory another tool cannot see all fail here.
//!
//! One catalog, the hand-written fixture. It declares `tags`, `requires` and `expects` between its
//! skills, which is the whole of what ambit adds to a frontmatter block, so one case covers every
//! key another tool's parser could choke on.
//!
//! **This is the one test allowed to reach the network** (nothing else in the suite may follow
//! it). Two consequences are deliberate. `@sentry/dotagents` is left unpinned, since the guarantee
//! is about the release people actually have rather than one frozen when this was written. And an
//! unreachable registry is a *skip*, with a printed reason, for a developer working offline, but a
//! failure when `CI` is set, because a compatibility test that quietly passed by never running is
//! worse than no test at all. `AMBIT_SKIP_NETWORK_TESTS=1` skips without even probing. Rust tests
//! have no skip status, so a skip is a printed reason and an early return.

#![allow(clippy::disallowed_methods)] // The test reads CI and the skip switch from the process, and lists with std::fs.

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

/// Unpinned on purpose: the promise is about whatever dotagents currently ships.
const DOTAGENTS_PACKAGE: &str = "@sentry/dotagents";

/// Set to skip the probe and the case outright: the offline developer's escape hatch.
const SKIP_VAR: &str = "AMBIT_SKIP_NETWORK_TESTS";

/// dotagents refuses a `path:` source resolving outside the project root, so the catalog under
/// test lives inside the project it is installed into.
const CATALOG_DIRNAME: &str = "catalog";

/// Where dotagents materializes skills, and the symlink it points each harness at.
const AGENTS_DIRNAME: &str = ".agents";
const INSTALLED_DIR: &str = ".agents/skills";
const CLAUDE_LINK: &str = ".claude/skills";

/// npm's retry-with-backoff is what turns "no network" into a minute of silence, so the child is
/// told to give up after one attempt, but only outside CI, where a transient registry blip
/// deserves a retry rather than a report that the promise is broken.
const IMPATIENT_NPM: &[(&str, &str)] = &[
    ("npm_config_fetch_retries", "0"),
    ("npm_config_fetch_timeout", "20000"),
];

/// A hard ceiling on every child, so an offline run ends in a message rather than a hang.
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);

/// How much of npm's own complaint to quote: enough to name the cause, not its stack trace.
const QUOTED_STDERR_LINES: usize = 3;

/// Whether `name` is set to something other than the empty string in the process environment.
fn is_set(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.is_empty())
}

/// What a child process did.
struct ChildResult {
    /// `None` when it was killed (by the ceiling) or could not be started.
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Runs `npx @sentry/dotagents <args>`, reporting how it went instead of panicking, so a failure
/// is an assertion naming the command's own output.
///
/// The cache and user-level install directories are redirected into `home`, because dotagents
/// defaults them under `$HOME` and a test that writes there is a test that changed the machine.
fn dotagents(args: &[&str], cwd: &Path, home: &Path) -> ChildResult {
    // npm ships npx as a batch file on Windows, and spawning resolves only `.exe` names itself.
    let mut command = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" });

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

    // The ceiling: a watcher waits on the child, and the test kills it if no answer comes in time.
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

/// Writes the project dotagents installs into: a wildcard entry over the catalog inside it, plus
/// the two `.gitignore` lines whose absence dotagents warns about (that warning is noise here, not
/// the subject).
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

/// The head of a failed child's complaint, since npm follows its reason with a stack trace.
fn first_lines(text: &str) -> String {
    text.split('\n')
        .filter(|line| !line.trim().is_empty())
        .take(QUOTED_STDERR_LINES)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether `npx @sentry/dotagents` runs at all, and why not when it does not. Doubles as the
/// warm-up: the install resolves from the npx cache this fills.
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
fn installs_every_skill_in_the_hand_written_fixture_catalog_ignoring_ambits_additions() {
    let home = tempdir();

    // Loud either way, for opposite reasons: offline, a developer needs to know the promise went
    // unchecked; in CI, a promise that quietly passed by never running is worse than no test.
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

    // `--project`, because dotagents operates on the global scope by default: a bare `install`
    // writes `~/.agents/agents.toml` and reports success, leaving the project untouched.
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

    // Every harness reads skills through this link, so an install that skipped it installed
    // nothing.
    assert!(
        fs::metadata(project.join(CLAUDE_LINK)).is_ok(),
        "{CLAUDE_LINK} is missing"
    );
}
