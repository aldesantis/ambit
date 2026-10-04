//! A disposable project pointed at the fixture catalog, for the install, harness and lock tests.
//!
//! Each test gets its own tempdir holding `catalog/` (the fixture, rebuilt) and `project/`, and an
//! environment whose `HOME` and cache live under that tempdir, so nothing touches the machine's own
//! and parallel tests never share a path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::errors::ExitCode;
use crate::model::catalog::{CatalogLoadOptions, load_catalogs, merge_catalogs};
use crate::model::config::load_project_config;
use crate::model::documents::{array_entry_key, managed_key};
use crate::model::sources::SourceContext;
use crate::model::state::{OwnedArtifact, STATE_DIRNAME, STATE_FILENAME, State, parse_state};
use crate::resolution::resolve::{Bundle, resolve_bundle};
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::{run_cli, tempdir, test_env};
use crate::util::env::Env;
use crate::util::fs::{self, EntryKind};
use crate::util::path::{join, normalize, relative, to_slash};

pub const CATALOG_NAME: &str = "company";
pub const SKILLS_DIR: &str = ".agents/skills";
pub const CLAUDE_LINK: &str = ".claude/skills";
pub const MCP_FILE: &str = ".mcp.json";
pub const CLAUDE_SETTINGS: &str = ".claude/settings.json";
pub const HOOK_DIR: &str = ".agents/hooks/guard-secrets";

pub const CORE_SKILL: &str = "company-context";
pub const ENGINEERING_SKILL: &str = "code-review";
pub const FRONTEND_SKILL: &str = "design-tokens";
pub const PROJECT_SKILL: &str = "acme-brief";

/// The fixture's tag-matched http server, and the one only `requires` reaches.
pub const PACKED_MCP: &str = "linter";
pub const FIXTURE_MCP: &str = "fixture";

/// The variable the packed server interpolates into its `Authorization` header.
pub const PACKED_KEY_VAR: &str = "LINTER_API_KEY";

/// The default profile: `function.engineering` also selects its nested frontend child, so this is
/// three skills, the packed server, and both fixture hooks.
pub const DEFAULT_PACKS: &[&str] = &["core", "function.engineering", "function.engineering.*"];

/// What one CLI run printed, with lines joined by `\n` and no trailing newline.
pub struct Output {
    pub code: ExitCode,
    pub stdout: String,
    pub stderr: String,
}

fn strip_last_newline(text: String) -> String {
    text.strip_suffix('\n').map(str::to_owned).unwrap_or(text)
}

/// The two managed keys a Claude-shaped hooks section holds for the fixture's hooks, in state's own
/// key order, with the script reached through `hooks_root`.
pub fn hook_keys(hooks_root: &str) -> Vec<String> {
    vec![engineering_hook_key(hooks_root), core_hook_key()]
}

/// The managed key of the inline-command hook `core` selects.
pub fn core_hook_key() -> String {
    managed_key(
        "hooks",
        &array_entry_key(
            "SessionStart",
            &serde_json::json!({
                "hooks": [{ "type": "command", "command": "echo \"acme conventions apply\"" }],
            }),
        ),
    )
}

/// The managed key of the script-shipping hook `function.engineering` selects.
pub fn engineering_hook_key(hooks_root: &str) -> String {
    managed_key(
        "hooks",
        &array_entry_key(
            "PreToolUse",
            &serde_json::json!({
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": format!("{hooks_root}/guard-secrets/guard.sh"),
                    "timeout": 10,
                }],
            }),
        ),
    )
}

/// Claude's spelling of the shared hooks directory.
pub const CLAUDE_HOOK_ROOT: &str = "${CLAUDE_PROJECT_DIR}/.agents/hooks";

pub struct Project {
    _temp: tempfile::TempDir,
    pub root: PathBuf,
    pub catalog: PathBuf,
    pub dir: PathBuf,
    pub env: Env,
}

impl Project {
    /// A fresh fixture catalog beside an empty project directory, with no `ambit.yml` yet.
    pub fn new() -> Self {
        let temp = tempdir();
        let root = temp.path().to_path_buf();
        let catalog = root.join("catalog");
        let dir = root.join("project");

        build_fixture_catalog(&catalog).expect("build the fixture catalog");
        fs::mkdir_p(&dir).expect("create the project");

        Self {
            env: test_env(&root),
            _temp: temp,
            root,
            catalog,
            dir,
        }
    }

    /// Points the project at the fixture catalog and gives it a `requires` list: one `pack:` entry
    /// per pack, then `entries` verbatim.
    pub fn write_profile(&self, packs: &[&str], harnesses: Option<&[&str]>, entries: &[&str]) {
        let mut written: Vec<String> = packs.iter().map(|pack| requires_entry(pack)).collect();

        written.extend(entries.iter().map(|&entry| entry.to_owned()));

        let list = if written.is_empty() {
            "[]".to_owned()
        } else {
            format!("\n{}", written.join("\n"))
        };
        let harness_line = harnesses.map_or_else(String::new, |harnesses| {
            format!("harnesses: [{}]\n", harnesses.join(", "))
        });

        self.write(
            "ambit.yml",
            &format!(
                "version: 1\n{harness_line}catalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nrequires: {list}\n"
            ),
        );
    }

    /// Runs `ambit <args> --project <dir>` from the tempdir root.
    pub fn cli(&self, args: &[&str]) -> Output {
        let dir = self.dir.to_string_lossy().into_owned();
        let mut argv: Vec<&str> = args.to_vec();

        argv.extend(["--project", dir.as_str()]);

        let result = run_cli(&argv, &self.root, &self.env);

        Output {
            code: result.code,
            stdout: strip_last_newline(result.stdout),
            stderr: strip_last_newline(result.stderr),
        }
    }

    /// An absolute path inside the project.
    pub fn path(&self, relative: &str) -> PathBuf {
        join(&self.dir, relative)
    }

    pub fn read(&self, relative: &str) -> String {
        read_file(&self.path(relative))
    }

    /// Writes a project file, creating its directory.
    pub fn write(&self, relative: &str, contents: &str) {
        write_file(&self.path(relative), contents);
    }

    /// Writes a file into this test's copy of the catalog.
    pub fn write_catalog(&self, relative: &str, contents: &str) {
        write_file(&join(&self.catalog, relative), contents);
    }

    /// Whether anything resolves at the path, following links (`stat`).
    pub fn exists(&self, relative: &str) -> bool {
        exists(&self.path(relative))
    }

    /// Whether anything sits at the path, links included even when dangling (`lstat`).
    pub fn lexists(&self, relative: &str) -> bool {
        fs::lstat_kind(&self.path(relative)).expect("lstat") != EntryKind::Missing
    }

    /// Where a symlink points, or `None` when the path is not one.
    pub fn link_at(&self, relative: &str) -> Option<String> {
        link_target(&self.path(relative))
    }

    /// Every file under `relative`, through links, `/`-separated and sorted.
    pub fn tree(&self, relative: &str) -> Vec<String> {
        let mut found = Vec::new();

        walk_tree(&self.path(relative), "", &mut found);
        found.sort();
        found
    }

    /// The installed skill directory names, sorted.
    pub fn installed_skills(&self) -> Vec<String> {
        let skills = self.path(SKILLS_DIR);
        let mut names: Vec<String> = fs::read_dir_names(&skills)
            .unwrap_or_default()
            .into_iter()
            .filter(|entry| is_dir(&skills.join(entry)))
            .collect();

        names.sort();
        names
    }

    /// Every file in the project, keyed by relative path and carrying its contents.
    ///
    /// A link pointing back inside the project is a second view of files this walk already has
    /// (`.claude/skills` is one, and following it would list every skill twice), so it is recorded
    /// as `-> <target>` and not descended into. A link out to the catalog is followed, because the
    /// bytes it exposes are only reachable through it.
    pub fn snapshot(&self) -> BTreeMap<String, String> {
        let mut found = BTreeMap::new();

        self.walk_snapshot(&self.dir.clone(), "", &mut found);
        found
    }

    fn walk_snapshot(&self, current: &Path, within: &str, found: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir_names(current).expect("list a directory") {
            let relative_path = if within.is_empty() {
                entry.clone()
            } else {
                format!("{within}/{entry}")
            };
            let absolute = current.join(&entry);

            if let Some(target) = link_target(&absolute) {
                let points = normalize(&current.join(&target));

                if !relative(&self.dir, &points).starts_with("..") {
                    found.insert(relative_path, format!("-> {target}"));
                    continue;
                }
            }

            if is_dir(&absolute) {
                self.walk_snapshot(&absolute, &relative_path, found);
            } else {
                found.insert(relative_path, read_file(&absolute));
            }
        }
    }

    pub fn read_state_file(&self) -> String {
        self.read(&format!("{STATE_DIRNAME}/{STATE_FILENAME}"))
    }

    /// The state file, parsed.
    pub fn state(&self) -> State {
        parse_state(&self.read_state_file(), STATE_FILENAME).expect("a valid state file")
    }

    pub fn state_artifacts(&self) -> Vec<OwnedArtifact> {
        self.state().artifacts
    }

    /// `.mcp.json` as a document.
    pub fn mcp_config(&self) -> serde_json::Value {
        serde_json::from_str(&self.read(MCP_FILE)).expect("valid JSON")
    }

    /// The bundle the project's current profile resolves to.
    pub fn bundle(&self) -> Bundle {
        let context = SourceContext {
            project_dir: self.dir.clone(),
            env: self.env.clone(),
            offline: false,
        };
        let config = load_project_config(&self.dir).expect("a valid config");
        let catalogs = load_catalogs(&config, &context, &mut CatalogLoadOptions::default())
            .expect("loadable catalogs");

        resolve_bundle(&config, &merge_catalogs(&catalogs)).expect("a resolvable profile")
    }
}

/// One `requires` entry, taking a whole pack from the fixture catalog.
pub fn requires_entry(pack: &str) -> String {
    requires_entry_from(pack, CATALOG_NAME)
}

/// One `requires` entry, taking a whole pack from `catalog`.
pub fn requires_entry_from(pack: &str, catalog: &str) -> String {
    format!("  - {{ pack: \"{catalog}/{pack}\" }}")
}

pub fn read_file(path: &Path) -> String {
    fs::read_text(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

pub fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::mkdir_p(parent).expect("create a parent directory");
    }

    fs::write_text(path, contents).expect("write a file");
}

/// Whether anything resolves at the path, following links (`stat`).
pub fn exists(path: &Path) -> bool {
    std::fs::metadata(path).is_ok()
}

/// Whether a path is a directory, *through* a symlink: a linked skill is a directory as far as the
/// harness reading it is concerned.
pub fn is_dir(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// Where a symlink points, or `None` when the path is not one.
pub fn link_target(path: &Path) -> Option<String> {
    if fs::lstat_kind(path).ok()? != EntryKind::Symlink {
        return None;
    }

    std::fs::read_link(path)
        .ok()
        .map(|target| to_slash(&target))
}

fn walk_tree(current: &Path, within: &str, found: &mut Vec<String>) {
    for entry in fs::read_dir_names(current).expect("list a directory") {
        let relative_path = if within.is_empty() {
            entry.clone()
        } else {
            format!("{within}/{entry}")
        };
        let absolute = current.join(&entry);

        if is_dir(&absolute) {
            walk_tree(&absolute, &relative_path, found);
        } else {
            found.push(relative_path);
        }
    }
}

/// A list of owned strings, for comparing against `Vec<String>`.
pub fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|&item| item.to_owned()).collect()
}

/// A whole-path artifact as state records it.
pub fn owned(path: &str, kind: crate::model::state::ArtifactKind, mode: &str) -> OwnedArtifact {
    OwnedArtifact {
        path: path.to_owned(),
        kind,
        mode: crate::model::state::ArtifactMode::parse(mode),
        managed_keys: None,
        format: None,
        shape: None,
        digest: None,
    }
}

/// A co-owned config file as state records it.
pub fn config(
    path: &str,
    format: crate::model::documents::DocumentFormat,
    shape: Option<crate::model::documents::DocumentShape>,
    keys: Vec<String>,
) -> OwnedArtifact {
    OwnedArtifact {
        path: path.to_owned(),
        kind: crate::model::state::ArtifactKind::HarnessConfig,
        mode: None,
        managed_keys: Some(keys),
        format: Some(format),
        shape,
        digest: None,
    }
}
