//! Builds the fixture catalog every test resolves against (everything must be testable offline).
//!
//! The catalog is a plain skills repo (skills at `skills/<name>/SKILL.md`, MCP entities at
//! `mcps/<name>.yml`, hooks at `hooks/<name>/hook.yml`, packs at `packs/<name>.yml`, and no config of
//! its own), so it doubles as the subject of the dotagents compatibility test: everything but
//! `skills/` is additive, and a tool that reads only skills must be unbothered by it.
//!
//! The packs are what selection runs through. Nothing in a catalog labels itself, so a consumer's
//! entry names either an item outright or a **pack** (a document that names the items), and the
//! fixture ships four, one of them nested two directories deep so the `packs/**` walk and the name
//! it derives are both exercised.
//!
//! It also builds that same catalog as a **local bare git repository**, which is how the git-source
//! tests stay offline: a `file://` URL is a git URL like any other, so nothing in ambit needs a test
//! mode to be exercised against one.
//!
//! Shared by the integration tests (`mod support;`) and the in-crate tests (`test_support`), so it
//! uses std and the `git` binary only, never ambit's own code.
#![allow(dead_code, clippy::disallowed_methods)] // Each including crate uses a subset; std::fs is the point here.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Written at the catalog root so a rebuild knows the directory is ours to delete. Dotfiles are
/// invisible to catalog parsing, which reads only `skills/**`, `mcps/*`, `hooks/**` and `packs/**`.
pub const FIXTURE_MARKER: &str = ".ambit-fixture";

const CORE_SKILL: &str = "---
name: company-context
description: Canonical context about Acme — what it sells, to whom, and how it works.
---

# Acme company context

Reached through the `core` pack, and pulled in by `requires` from the project skill even when
nothing selects that pack at all.
";

const ENGINEERING_SKILL: &str = "---
name: code-review
description: How Acme reviews code — what reviewers look for, and in what order.
---

# Code review at Acme

A member of the `function.engineering` pack, and of no other. The `core` pack must not reach it.
";

const FRONTEND_SKILL: &str = "---
name: design-tokens
description: Acme's design tokens — color, spacing, and the type scale.
ambit:
  expects:
    - env: ACME_FIGMA_TOKEN
---

# Design tokens

Belongs to `function.engineering.frontend`, one dot below `function.engineering` and reached by
neither that pack nor an exact-name entry for it: a pattern without a `*` is an exact match, so
`pack: function.engineering` leaves this behind and `pack: function.engineering.*` leaves the
narrower one's parent behind. Both together is two entries. The dot is a character, not a level.
";

const PROJECT_SKILL: &str = "---
name: acme-brief
description: The Acme engagement brief — remit, contacts, and conventions.
ambit:
  # Unqualified, because a catalog author cannot write a consumer's alias — so each entry resolves
  # within this catalog. Exact names here: a pattern with no wildcard is one item, exactly.
  requires:
    - skill: company-context
    - mcp: fixture
    - hook: acme-standup
---

# Acme engagement brief

Reaches a skill, an MCP server and a hook that no entry a test writes selects on its own, so the
`requires` closure is the only thing that can pull them in — a skill's `requires` and a pack's
being the same grammar and the same closure.
";

const REQUIRED_MCP: &str = "name: fixture
# In no pack: reachable only because acme-brief requires it.

transport:
  stdio:
    command: npx
    args: [\"-y\", \"@acme/fixture-mcp\"]

expects:
  - env: FIXTURE_API_KEY
";

const PACKED_MCP: &str = "name: linter
# A member of the `function.engineering` pack, which is the only thing that reaches it.

transport:
  http:
    url: https://mcp.invalid/fixture
    bearer_token_env_var: LINTER_API_KEY

expects:
  - env: LINTER_API_KEY
";

const COMMAND_HOOK: &str = "name: session-notes
description: Reminds a session that Acme's conventions apply.

event: SessionStart
# `type: command` means the harness runs this verbatim and nothing is looked for on disk. The
# hook's directory holds nothing but this file.
type: command
command: echo \"acme conventions apply\"
";

const SCRIPT_HOOK: &str = "name: guard-secrets
description: Inspects a Bash command before Acme's tooling runs it.

event: PreToolUse
matcher: Bash
# `type: script` names `guard.sh`, which this directory ships, so the script is materialized
# under `.agents/hooks/guard-secrets/` and the command is rewritten to point at it.
type: script
command: guard.sh
timeout: 10
";

const HOOK_SCRIPT: &str = "#!/bin/sh
# Shipped by hooks/guard-secrets, which is what makes it the fixture's only hook that installs
# bytes rather than config values. Inert on purpose: it reads the tool call it is handed and
# allows it, so what is being exercised is materialization and not the guard.
cat >/dev/null
exit 0
";

const REQUIRED_HOOK: &str = "name: acme-standup
# In no pack: reachable only because acme-brief requires it.
description: Records what the session touched, for the Acme standup.

event: SessionEnd
type: command
command: echo \"acme session ended\"
";

const CORE_PACK: &str = "name: core
description: What every Acme session needs, whoever is in it.

requires:
  - skill: company-context
  - hook: session-notes
";

const ENGINEERING_PACK: &str = "name: function.engineering
description: Everything an Acme engineer needs — reviews, tooling, and the guards around them.

# A pack requires other packs as readily as it requires items, which is what lets a catalog build a
# large grouping out of small ones instead of restating the small one's membership.
requires:
  - pack: core
  - skill: code-review
  - mcp: linter
  - hook: guard-secrets
";

const FRONTEND_PACK: &str = "name: function.engineering.frontend
description: What an Acme engineer working on interfaces needs on top of the engineering pack.

requires:
  - pack: function.engineering
  - skill: design-tokens
";

const PROJECT_PACK: &str = "name: project.acme
description: The Acme engagement — its brief, and whatever the brief drags in.

requires:
  - skill: acme-brief
";

/// Every file in the fixture, keyed by its path relative to the catalog root.
pub const FIXTURE_CATALOG_FILES: &[(&str, &str)] = &[
    (
        FIXTURE_MARKER,
        "generated by tests/support/fixture_catalog.rs — safe to delete\n",
    ),
    ("hooks/session-notes/hook.yml", COMMAND_HOOK),
    ("hooks/guard-secrets/hook.yml", SCRIPT_HOOK),
    ("hooks/guard-secrets/guard.sh", HOOK_SCRIPT),
    ("hooks/acme-standup/hook.yml", REQUIRED_HOOK),
    ("mcps/fixture.yml", REQUIRED_MCP),
    ("mcps/linter.yml", PACKED_MCP),
    // Flat, nested one deep, and nested two deep, so the `packs/**` walk and the name it derives
    // from a path are both exercised by the catalog every other test resolves against.
    ("packs/core.yml", CORE_PACK),
    ("packs/function/engineering.yml", ENGINEERING_PACK),
    ("packs/function/engineering/frontend.yml", FRONTEND_PACK),
    ("packs/project/acme.yml", PROJECT_PACK),
    ("skills/company-context/SKILL.md", CORE_SKILL),
    ("skills/code-review/SKILL.md", ENGINEERING_SKILL),
    ("skills/design-tokens/SKILL.md", FRONTEND_SKILL),
    ("skills/acme-brief/SKILL.md", PROJECT_SKILL),
];

/// The files the fixture writes executable rather than `0o644`.
///
/// A hook script the harness cannot execute is a hook that does not run (Claude Code dispatches it
/// and `/bin/sh` answers `Permission denied`), so the bit is part of what a catalog ships, exactly
/// like the bytes.
pub const FIXTURE_EXECUTABLE_FILES: &[&str] = &["hooks/guard-secrets/guard.sh"];

/// Clears the target so a rebuild cannot leave stale files behind, refusing any directory the
/// builder did not create.
fn clear_target(dir: &Path) -> io::Result<()> {
    let metadata = match fs::metadata(dir) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };

    if !metadata.is_dir() {
        return Err(io::Error::other(format!(
            "fixture target is not a directory: {}",
            dir.display()
        )));
    }

    let empty = fs::read_dir(dir)?.next().is_none();

    if !empty && !dir.join(FIXTURE_MARKER).exists() {
        return Err(io::Error::other(format!(
            "refusing to overwrite {}: it is not empty and has no {FIXTURE_MARKER} marker",
            dir.display()
        )));
    }

    fs::remove_dir_all(dir)
}

#[cfg(unix)]
fn make_executable(target: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(target, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_target: &Path) -> io::Result<()> {
    Ok(())
}

/// Writes the fixture catalog into `dir`, replacing any previous build. Idempotent: the same `dir`
/// always ends up with byte-identical contents and nothing extra.
///
/// Returns the catalog root, absolute.
///
/// # Errors
///
/// Refuses a non-empty directory without the marker, and any I/O error.
pub fn build_fixture_catalog(dir: &Path) -> io::Result<PathBuf> {
    let root = std::path::absolute(dir)?;

    clear_target(&root)?;

    let mut files: Vec<(&str, &str)> = FIXTURE_CATALOG_FILES.to_vec();
    files.sort_by(|a, b| a.0.cmp(b.0));

    for (relative, contents) in files {
        let target = root.join(relative);

        fs::create_dir_all(target.parent().expect("every fixture file has a parent"))?;
        fs::write(&target, contents)?;

        if FIXTURE_EXECUTABLE_FILES.contains(&relative) {
            make_executable(&target)?;
        }
    }

    Ok(root)
}

/// The branch the fixture repository's `HEAD` points at, so an absent `ref` finds something.
pub const FIXTURE_GIT_BRANCH: &str = "main";

/// A tag on the same commit, so a test can ask for a ref that is not a branch.
pub const FIXTURE_GIT_TAG: &str = "v1";

/// The bare repository, within the directory the builder is given.
const BARE_DIRNAME: &str = "catalog.git";

/// The working tree the bare repository is cloned from.
const WORK_DIRNAME: &str = "catalog-work";

/// Fixed identity and dates, and neither user nor system config, so the fixture's commit SHA is the
/// same in every run on every machine, which is what lets a test name the cache path it produces.
const FIXTURE_GIT_ENV: &[(&str, &str)] = &[
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_AUTHOR_NAME", "ambit fixtures"),
    ("GIT_AUTHOR_EMAIL", "fixtures@ambit.invalid"),
    ("GIT_AUTHOR_DATE", "2024-01-01T00:00:00+00:00"),
    ("GIT_COMMITTER_NAME", "ambit fixtures"),
    ("GIT_COMMITTER_EMAIL", "fixtures@ambit.invalid"),
    ("GIT_COMMITTER_DATE", "2024-01-01T00:00:00+00:00"),
];

/// The fixture catalog as a git repository ambit can fetch without a network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixtureGitCatalog {
    /// Absolute path to the bare repository.
    pub repo: PathBuf,
    /// A `file://` URL for it: a git URL like any other, so no test mode is needed.
    pub url: String,
    /// The commit both the branch and the tag point at.
    pub commit: String,
    pub branch: String,
    pub tag: String,
    /// The working tree the bare repository was cloned from, so a test can commit a second
    /// revision.
    pub work: PathBuf,
}

/// Runs git with the fixture identity and returns its trimmed stdout.
///
/// # Errors
///
/// A spawn failure, or a non-zero exit carrying git's stderr.
pub fn git<S: AsRef<std::ffi::OsStr>>(args: &[S], cwd: &Path) -> io::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .envs(FIXTURE_GIT_ENV.iter().copied())
        .stdin(Stdio::null())
        .output()?;

    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The `file://` URL git accepts for a local path, `/`-separated on every platform.
pub fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");

    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

/// Builds the fixture catalog as a bare git repository inside `dir`, replacing any previous build.
///
/// The tree committed is exactly [`build_fixture_catalog`]'s, so a git-source install and a
/// `path:`-source install of the same fixture must produce identical results.
///
/// # Errors
///
/// Any I/O or git failure.
pub fn build_fixture_git_catalog(dir: &Path) -> io::Result<FixtureGitCatalog> {
    let root = std::path::absolute(dir)?;
    let work = root.join(WORK_DIRNAME);
    let repo = root.join(BARE_DIRNAME);

    remove_if_present(&work)?;
    remove_if_present(&repo)?;
    build_fixture_catalog(&work)?;

    git(&["init", "--quiet"], &work)?;
    // `symbolic-ref` rather than `init -b`: it names the initial branch the same way to every git.
    git(
        &[
            "symbolic-ref",
            "HEAD",
            &format!("refs/heads/{FIXTURE_GIT_BRANCH}"),
        ],
        &work,
    )?;
    git(&["add", "--all"], &work)?;

    // Set in the index, not only on disk: git on Windows does not read the exec bit from the
    // filesystem, and the commit must be the same on every machine.
    for executable in FIXTURE_EXECUTABLE_FILES {
        git(&["update-index", "--chmod=+x", "--", executable], &work)?;
    }

    git(
        &["commit", "--quiet", "--message", "the fixture catalog"],
        &work,
    )?;
    git(&["tag", FIXTURE_GIT_TAG], &work)?;
    let commit = git(&["rev-parse", "HEAD"], &work)?;

    git(
        &[
            std::ffi::OsStr::new("clone"),
            "--mirror".as_ref(),
            "--quiet".as_ref(),
            "--".as_ref(),
            work.as_os_str(),
            repo.as_os_str(),
        ],
        &root,
    )?;

    Ok(FixtureGitCatalog {
        url: file_url(&repo),
        repo,
        commit,
        branch: FIXTURE_GIT_BRANCH.to_owned(),
        tag: FIXTURE_GIT_TAG.to_owned(),
        work,
    })
}

fn remove_if_present(target: &Path) -> io::Result<()> {
    match fs::remove_dir_all(target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Commits an edit to the fixture repository and pushes it, moving the branch.
///
/// What `ambit outdated` and `ambit update` need and no other suite does: a repository whose `ref`
/// points somewhere new since a project last resolved it. The working tree is reused, so the second
/// commit's parent is the first and the branch fast-forwards exactly as a real one would. The tag is
/// deliberately left where it was, so one repository can serve both a moving ref and a standing one.
///
/// `files` is the edit, keyed by repo-relative `/`-separated path; `None` deletes a file. Returns
/// the commit the branch now points at.
///
/// # Errors
///
/// Any I/O or git failure.
pub fn commit_fixture_git_revision(
    fixture: &FixtureGitCatalog,
    files: &[(&str, Option<&str>)],
    message: &str,
) -> io::Result<String> {
    for (relative, text) in files {
        let target = fixture.work.join(relative);

        match text {
            None => {
                if target.is_dir() {
                    remove_if_present(&target)?;
                } else if target.exists() {
                    fs::remove_file(&target)?;
                }
            }
            Some(text) => {
                fs::create_dir_all(target.parent().expect("a repo-relative path has a parent"))?;
                fs::write(&target, text)?;
            }
        }
    }

    git(&["add", "--all"], &fixture.work)?;
    git(&["commit", "--quiet", "--message", message], &fixture.work)?;
    let commit = git(&["rev-parse", "HEAD"], &fixture.work)?;

    git(
        &[
            std::ffi::OsStr::new("push"),
            "--quiet".as_ref(),
            fixture.repo.as_os_str(),
            FIXTURE_GIT_BRANCH.as_ref(),
        ],
        &fixture.work,
    )?;

    Ok(commit)
}
