//! `ambit prune` and `ambit clean`: the two commands that only remove.
//!
//! Every case asserts both directions, because half of each claim is about restraint. A prune that
//! removed everything and a prune that removed nothing would each satisfy "the withdrawn skill is
//! gone" or "the selected ones are still there" on its own, so both are pinned every time, and the
//! same goes for what neither command owns: a hand-written skill, a hand-added server, and the
//! files ambit writes but does not own.
//!
//! The install-time prune is pinned in the install tests; what these add is the part only a
//! standalone command has: that it reaches the same set without materializing anything, that it
//! rewrites the records afterwards, and that a run with nothing to do writes nothing at all.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::errors::ExitCode;
use crate::model::documents::{array_entry_key, managed_key};
use crate::model::state::{STATE_FILENAME, parse_state};
use crate::project::doctor::{DoctorOptions, diagnose_project, is_healthy};
use crate::project::gitignore::{BLOCK_BEGIN, BLOCK_END};
use crate::project::lock::LOCK_FILENAME;
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::{CliResult, run_cli, tempdir, test_env};
use crate::util::fs::{read_dir_names, read_text};
use crate::util::text::pad_end;

const CATALOG_NAME: &str = "company";
const SKILLS_DIR: &str = ".agents/skills";
const CLAUDE_LINK: &str = ".claude/skills";
const MCP_FILE: &str = ".mcp.json";

const CORE_SKILL: &str = "company-context";
const ENGINEERING_SKILL: &str = "code-review";
const FRONTEND_SKILL: &str = "design-tokens";

/// The fixture's tag-matched http server.
const PACKED_MCP: &str = "linter";

/// The fixture's two tag-matched hooks share this file.
///
/// `core` selects an inline-command hook and `function.engineering` a script-shipping one, so the
/// bundle these cases install carries a config file and a materialized directory neither skills nor
/// servers account for.
const CLAUDE_SETTINGS: &str = ".claude/settings.json";
const SCRIPT_HOOK_DIR: &str = ".agents/hooks/guard-secrets";

const HANDMADE_SKILL: &str = "hand-written";

/// The default profile: three skills (`function.engineering` also selects its nested frontend
/// child) plus the tagged http server, which declares that same tag.
const DEFAULT_PACKS: &[&str] = &["core", "function.engineering", "function.engineering.*"];

fn state_file() -> String {
    format!("{STATE_DIRNAME}/{STATE_FILENAME}")
}

/// The managed keys of the fixture's two hooks.
///
/// Built from the rendered entry rather than written out, because a digest is not a literal anyone
/// can check by eye; the hooks tests are where the rendering itself is pinned.
fn core_hook_key() -> String {
    managed_key(
        "hooks",
        &array_entry_key(
            "SessionStart",
            &json!({
                "hooks": [{ "type": "command", "command": "echo \"acme conventions apply\"" }],
            }),
        ),
    )
}

fn engineering_hook_key() -> String {
    managed_key(
        "hooks",
        &array_entry_key(
            "PreToolUse",
            &json!({
                "matcher": "Bash",
                "hooks": [{
                    "type": "command",
                    "command": format!("${{CLAUDE_PROJECT_DIR}}/{SCRIPT_HOOK_DIR}/guard.sh"),
                    "timeout": 10,
                }],
            }),
        ),
    )
}

/// One `requires` entry, taking a whole pack from the fixture catalog.
fn requires_entry(pack: &str) -> String {
    format!("  - {{ pack: \"{CATALOG_NAME}/{pack}\" }}")
}

fn profile_text(packs: &[&str]) -> String {
    let list = if packs.is_empty() {
        "[]".to_owned()
    } else {
        format!(
            "\n{}",
            packs
                .iter()
                .map(|pack| requires_entry(pack))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };

    format!(
        "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nrequires: {list}\n"
    )
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    catalog_dir: PathBuf,
    project_dir: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempdir();
    let root = dir.path().to_path_buf();
    let catalog_dir = root.join("catalog");
    let project_dir = root.join("project");
    build_fixture_catalog(&catalog_dir).unwrap();
    fs::create_dir_all(&project_dir).unwrap();

    let f = Fixture {
        _dir: dir,
        root,
        catalog_dir,
        project_dir,
    };
    // The tagged server interpolates `LINTER_API_KEY` into a header; `test_env` never sets it, so
    // what is on disk does not depend on the machine.
    f.write_profile(DEFAULT_PACKS);
    f
}

/// `ambit clean`'s cases all start from an installed default profile.
fn installed() -> Fixture {
    let f = fixture();
    let result = f.cli(&["install"]);
    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    f
}

impl Fixture {
    /// Points the project at the fixture catalog and gives it a `requires` list.
    fn write_profile(&self, packs: &[&str]) {
        fs::write(self.project_dir.join("ambit.yml"), profile_text(packs)).unwrap();
    }

    fn env(&self) -> crate::util::env::Env {
        test_env(&self.root)
    }

    /// Runs the CLI against the project, collecting stdout and stderr.
    fn cli(&self, argv: &[&str]) -> CliResult {
        let project = self.project_dir.to_string_lossy().into_owned();
        let mut args = argv.to_vec();
        args.extend(["--project", &project]);

        run_cli(&args, &self.root, &self.env())
    }

    fn read(&self, file: &str) -> String {
        read_text(&self.project_dir.join(file)).unwrap()
    }

    /// Every file in the project, keyed by relative path and carrying its contents.
    ///
    /// Symlinks are followed, because the default install of a `path:` catalog is a link and what
    /// these tests compare is the files a harness would read.
    fn snapshot(&self) -> BTreeMap<String, String> {
        fn walk(current: &Path, relative: &str, found: &mut BTreeMap<String, String>) {
            for entry in read_dir_names(current).unwrap() {
                let within = if relative.is_empty() {
                    entry.clone()
                } else {
                    format!("{relative}/{entry}")
                };
                let absolute = current.join(&entry);

                if fs::metadata(&absolute).unwrap().is_dir() {
                    walk(&absolute, &within, found);
                } else {
                    found.insert(within, read_text(&absolute).unwrap());
                }
            }
        }

        let mut found = BTreeMap::new();
        walk(&self.project_dir, "", &mut found);
        found
    }

    /// The installed skill directory names, sorted.
    fn installed_skills(&self) -> Vec<String> {
        let mut names = read_dir_names(&self.project_dir.join(SKILLS_DIR)).unwrap_or_default();
        names.sort();
        names
    }

    fn path_exists(&self, target: &str) -> bool {
        fs::metadata(self.project_dir.join(target)).is_ok()
    }

    fn read_mcp_config(&self) -> Value {
        serde_json::from_str(&self.read(MCP_FILE)).unwrap()
    }

    fn owned_paths_now(&self) -> Vec<String> {
        parse_state(&self.read(&state_file()), STATE_FILENAME)
            .unwrap()
            .artifacts
            .into_iter()
            .map(|artifact| artifact.path)
            .collect()
    }

    /// The lines between one file's markers, or `None` when it holds no block.
    fn managed_block(&self, file: &str) -> Option<Vec<String>> {
        let text = read_text(&self.project_dir.join(file)).ok()?;
        let lines: Vec<&str> = text.split('\n').collect();
        let start = lines
            .iter()
            .position(|line| line.starts_with(BLOCK_BEGIN))?;
        let end = lines.iter().position(|line| line.starts_with(BLOCK_END))?;

        if end <= start {
            return None;
        }

        Some(
            lines[start + 1..end]
                .iter()
                .map(|&line| line.to_owned())
                .collect(),
        )
    }

    /// The lock a clean install of `packs` writes, produced in a throwaway sibling project.
    ///
    /// Comparing against this rather than a hand-written expectation makes the claim the fix is
    /// about: what a prune leaves behind is what install would have written for the surviving set,
    /// not a document prune assembled by subtracting from the old one. The sibling sits beside the
    /// project so its `path:../catalog` names the same fixture.
    fn lock_of_fresh_install(&self, name: &str, packs: &[&str]) -> String {
        let reference = self.root.join(name);
        fs::create_dir_all(&reference).unwrap();
        fs::write(reference.join("ambit.yml"), profile_text(packs)).unwrap();

        let result = run_cli(
            &["install", "--project", &reference.to_string_lossy()],
            &self.root,
            &self.env(),
        );

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        read_text(&reference.join(LOCK_FILENAME)).unwrap()
    }

    /// A skill directory beside ambit's that no state claims.
    fn write_foreign_skill_dir(&self) {
        let target = self.project_dir.join(SKILLS_DIR).join(HANDMADE_SKILL);
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("SKILL.md"),
            format!("---\nname: {HANDMADE_SKILL}\n---\n"),
        )
        .unwrap();
    }
}

fn sorted(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|&name| name.to_owned()).collect();
    names.sort();
    names
}

fn lines_out(lines: &[String]) -> String {
    lines.iter().map(|line| line.clone() + "\n").collect()
}

fn pretty(value: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(value).unwrap())
}

mod ambit_prune {
    use super::*;

    #[test]
    fn removes_the_skills_the_narrowed_bundle_dropped_and_keeps_the_ones_it_still_selects() {
        let f = fixture();
        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        f.write_profile(&["core"]);

        let result = f.cli(&["prune"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(f.installed_skills(), [CORE_SKILL]);
        assert_eq!(
            f.read(&format!("{SKILLS_DIR}/{CORE_SKILL}/SKILL.md")),
            read_text(&f.catalog_dir.join("skills/company-context/SKILL.md")).unwrap()
        );
    }

    #[test]
    fn stops_claiming_what_it_removed_and_rewrites_the_managed_blocks_to_match() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&["core"]);

        f.cli(&["prune"]);

        // The settings file stays owned: the narrowed profile still holds `core`, whose hook it
        // carries.
        assert_eq!(
            f.owned_paths_now(),
            [
                format!("{SKILLS_DIR}/{CORE_SKILL}"),
                CLAUDE_SETTINGS.to_owned(),
                CLAUDE_LINK.to_owned(),
            ]
        );
        assert_eq!(
            f.managed_block(SHARED_GITIGNORE_FILE),
            Some(vec![format!("/skills/{CORE_SKILL}")])
        );
        assert_eq!(
            f.managed_block(GITIGNORE_FILENAME),
            Some(vec![format!("{STATE_DIRNAME}/"), CLAUDE_LINK.to_owned()])
        );
    }

    #[test]
    fn removes_the_server_keys_the_narrowed_bundle_dropped_and_keeps_the_file() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&["core"]);

        f.cli(&["prune"]);

        // ambit owns keys in this file and not the document, so the section empties and the file
        // stays, and state stops claiming it at all.
        assert_eq!(f.read_mcp_config(), json!({ "mcpServers": {} }));
        assert!(!f.owned_paths_now().contains(&MCP_FILE.to_owned()));
    }

    #[test]
    fn leaves_a_hand_written_skill_and_a_hand_added_server_exactly_where_they_are() {
        let f = fixture();
        fs::write(
            f.project_dir.join(MCP_FILE),
            pretty(&json!({
                "mcpServers": { "handmade": { "command": "node" } },
                "extra": { "kept": true },
            })),
        )
        .unwrap();
        f.cli(&["install"]);
        f.write_foreign_skill_dir();
        f.write_profile(&["core"]);

        let result = f.cli(&["prune"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(f.installed_skills(), sorted(&[CORE_SKILL, HANDMADE_SKILL]));
        assert_eq!(
            f.read_mcp_config(),
            json!({
                "mcpServers": { "handmade": { "command": "node" } },
                "extra": { "kept": true },
            })
        );
    }

    #[test]
    fn rewrites_ambit_lock_to_the_bundle_it_pruned_down_to_byte_for_byte_as_install_would() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&["core"]);

        f.cli(&["prune"]);

        // The reference is a fresh install of the narrowed profile into a second project: the lock
        // a prune leaves must be the one install writes for the surviving set, not a subtraction of
        // its own.
        let pruned = f.read(LOCK_FILENAME);

        assert_eq!(pruned, f.lock_of_fresh_install("reference-a", &["core"]));

        // And it really did change; otherwise the assertion above would pass on a prune that wrote
        // nothing.
        assert_ne!(
            pruned,
            f.lock_of_fresh_install("reference-b", DEFAULT_PACKS)
        );
        assert!(!pruned.contains(FRONTEND_SKILL));
        assert!(!pruned.contains(PACKED_MCP));
    }

    #[test]
    fn leaves_the_lock_byte_identical_when_it_prunes_nothing_rather_than_rewriting_it_in_place() {
        let f = fixture();
        f.cli(&["install"]);
        let lock = f.read(LOCK_FILENAME);

        f.cli(&["prune"]);

        assert_eq!(f.read(LOCK_FILENAME), lock);
    }

    #[test]
    fn writes_no_lock_under_dry_run_however_much_it_says_it_would_remove() {
        let f = fixture();
        f.cli(&["install"]);
        let lock = f.read(LOCK_FILENAME);

        f.write_profile(&["core"]);

        let result = f.cli(&["prune", "--dry-run"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(f.read(LOCK_FILENAME), lock);
    }

    #[test]
    fn leaves_a_pruned_project_passing_ambit_doctor_lock_check_included() {
        let f = fixture();
        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        f.write_profile(&["core"]);

        assert_eq!(f.cli(&["prune"]).code, ExitCode::Success);

        // The whole point of rewriting the lock: the project a prune leaves is one `doctor` calls
        // healthy, where it used to report `ambit.lock is out of date` for the change the prune had
        // just made.
        let report = diagnose_project(&f.project_dir, &f.env(), DoctorOptions::default()).unwrap();
        let findings: Vec<String> = report
            .findings
            .iter()
            .map(|finding| format!("{}/{}", finding.check, finding.severity))
            .collect();

        assert!(findings.is_empty(), "{findings:?}");
        assert!(is_healthy(&report));
    }

    #[test]
    fn changes_no_bytes_when_the_bundle_is_unchanged() {
        let f = fixture();
        f.cli(&["install"]);
        let before = f.snapshot();

        let result = f.cli(&["prune"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(f.snapshot(), before);
        assert_eq!(
            f.installed_skills(),
            sorted(&[ENGINEERING_SKILL, CORE_SKILL, FRONTEND_SKILL])
        );
    }

    #[test]
    fn writes_nothing_at_all_in_a_project_ambit_never_installed_into() {
        let f = fixture();
        let result = f.cli(&["prune"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        // Not even the state file or a `.gitignore` block: a prune with nothing to remove has
        // nothing to record either, and creating them here would be claiming an install that never
        // happened.
        assert_eq!(f.snapshot().into_keys().collect::<Vec<_>>(), ["ambit.yml"]);
    }

    #[test]
    fn is_a_no_op_the_second_time() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&["core"]);
        f.cli(&["prune"]);
        let before = f.snapshot();

        let result = f.cli(&["prune"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(f.snapshot(), before);
        assert_eq!(result.stdout, "pruned (0)\n  (none)\n");
    }

    #[test]
    fn lists_what_it_removed() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&["core"]);

        let result = f.cli(&["prune"]);

        let width = format!("{SKILLS_DIR}/{FRONTEND_SKILL}").len();
        let pad = |text: &str| pad_end(text, width);

        assert_eq!(
            result.stdout,
            lines_out(&[
                "pruned (5)".to_owned(),
                format!("  {}  hook-dir        -", pad(SCRIPT_HOOK_DIR)),
                format!(
                    "  {}  skill-dir       -",
                    pad(&format!("{SKILLS_DIR}/{ENGINEERING_SKILL}"))
                ),
                format!(
                    "  {}  skill-dir       -",
                    pad(&format!("{SKILLS_DIR}/{FRONTEND_SKILL}"))
                ),
                format!(
                    "  {}  harness-config  {}",
                    pad(CLAUDE_SETTINGS),
                    engineering_hook_key()
                ),
                format!(
                    "  {}  harness-config  mcpServers.{PACKED_MCP}",
                    pad(MCP_FILE)
                ),
            ])
        );
    }

    #[test]
    fn emits_what_it_removed_and_what_it_still_owns_carrying_no_absolute_paths() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&["core"]);

        let result = f.cli(&["prune", "--json"]);
        let report: Value = serde_json::from_str(&result.stdout).unwrap();

        assert_eq!(
            report,
            json!({
                "pruned": [
                    { "kind": "hook-dir", "path": SCRIPT_HOOK_DIR },
                    { "kind": "skill-dir", "path": format!("{SKILLS_DIR}/{ENGINEERING_SKILL}") },
                    { "kind": "skill-dir", "path": format!("{SKILLS_DIR}/{FRONTEND_SKILL}") },
                    {
                        "kind": "harness-config",
                        "managedKeys": [engineering_hook_key()],
                        "path": CLAUDE_SETTINGS,
                    },
                    {
                        "kind": "harness-config",
                        "managedKeys": [format!("mcpServers.{PACKED_MCP}")],
                        "path": MCP_FILE,
                    },
                ],
                // Neither the link nor the settings file is pruned: a narrowed profile still holds
                // skills, so it still points at them, and still holds the hook the file's remaining
                // entry is.
                "remaining": [
                    { "kind": "skill-dir", "mode": "link", "path": format!("{SKILLS_DIR}/{CORE_SKILL}") },
                    { "kind": "harness-config", "managedKeys": [core_hook_key()], "path": CLAUDE_SETTINGS },
                    { "kind": "skills-link", "mode": "link", "path": CLAUDE_LINK },
                ],
            })
        );
        assert!(!result.stdout.contains(&*f.root.to_string_lossy()));
    }

    #[test]
    fn reports_what_it_would_remove_under_dry_run_and_removes_none_of_it() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&["core"]);
        let before = f.snapshot();

        let result = f.cli(&["prune", "--dry-run", "--json"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        let report: Value = serde_json::from_str(&result.stdout).unwrap();

        assert_eq!(
            report["pruned"],
            json!([
                { "kind": "hook-dir", "path": SCRIPT_HOOK_DIR },
                { "kind": "skill-dir", "path": format!("{SKILLS_DIR}/{ENGINEERING_SKILL}") },
                { "kind": "skill-dir", "path": format!("{SKILLS_DIR}/{FRONTEND_SKILL}") },
                {
                    "kind": "harness-config",
                    "managedKeys": [engineering_hook_key()],
                    "path": CLAUDE_SETTINGS,
                },
                {
                    "kind": "harness-config",
                    "managedKeys": [format!("mcpServers.{PACKED_MCP}")],
                    "path": MCP_FILE,
                },
            ])
        );
        assert_eq!(f.snapshot(), before);
    }

    #[test]
    fn removes_nothing_when_the_project_resolves_to_what_is_already_installed() {
        let f = fixture();
        f.cli(&["install"]);

        let result = prune_project(&f.project_dir, &f.env(), PruneOptions::default()).unwrap();

        assert_eq!(result.pruned.len(), 0);
    }
}

mod ambit_clean {
    use super::*;

    #[test]
    fn removes_every_skill_directory_and_every_managed_server_key() {
        let f = installed();
        let result = f.cli(&["clean"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(f.installed_skills().len(), 0);
        assert_eq!(f.read_mcp_config(), json!({ "mcpServers": {} }));
    }

    #[test]
    fn removes_ambits_own_state_directory_and_both_managed_blocks() {
        let f = installed();
        f.cli(&["clean"]);

        assert!(!f.path_exists(STATE_DIRNAME));
        assert_eq!(f.managed_block(GITIGNORE_FILENAME), None);
        // Its block was the whole of the nested file, so the file goes with it.
        assert!(!f.path_exists(SHARED_GITIGNORE_FILE));
    }

    #[test]
    fn leaves_the_project_holding_only_the_files_ambit_does_not_own() {
        let f = installed();
        f.cli(&["clean"]);

        // `ambit.lock` is a record of a resolution rather than an artifact, and `.mcp.json` is
        // co-owned, so neither is ambit's to delete (see `project/clean.rs`). The `.gitignore`
        // ambit created goes, because ambit's block was the whole of it.
        assert_eq!(
            f.snapshot().into_keys().collect::<Vec<_>>(),
            sorted(&["ambit.yml", "ambit.lock", MCP_FILE, CLAUDE_SETTINGS])
        );
    }

    #[test]
    fn gives_a_gitignore_the_project_already_had_back_byte_for_byte() {
        let f = installed();
        let handwritten = "node_modules/\n.env\n";

        fs::remove_file(f.project_dir.join(GITIGNORE_FILENAME)).unwrap();
        fs::write(f.project_dir.join(GITIGNORE_FILENAME), handwritten).unwrap();
        f.cli(&["install"]);

        f.cli(&["clean"]);

        // The blank line above the block was ambit's separator, so it goes with the block.
        assert_eq!(f.read(GITIGNORE_FILENAME), handwritten);
    }

    #[test]
    fn leaves_a_hand_written_skill_and_a_hand_added_server_untouched() {
        let f = installed();
        f.write_foreign_skill_dir();
        let handmade = json!({ "command": "node", "args": ["./scripts/local-mcp.js"] });

        fs::write(
            f.project_dir.join(MCP_FILE),
            pretty(&json!({
                "mcpServers": { "handmade": handmade, PACKED_MCP: { "type": "http", "url": "x" } },
                "extra": 1,
            })),
        )
        .unwrap();
        // The tagged key is ambit's, so re-installing over the hand-edited file keeps ownership of
        // it.
        f.cli(&["install", "--adopt"]);

        let result = f.cli(&["clean"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(f.installed_skills(), [HANDMADE_SKILL]);
        assert_eq!(
            f.read_mcp_config(),
            json!({ "mcpServers": { "handmade": handmade }, "extra": 1 })
        );
    }

    #[test]
    fn works_on_a_project_whose_catalog_can_no_longer_be_resolved() {
        // The whole point of answering from state alone: this is the state a project is usually in
        // when someone reaches for `clean`.
        let f = installed();
        fs::remove_file(f.project_dir.join("ambit.yml")).unwrap();

        let result = f.cli(&["clean"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(f.installed_skills().len(), 0);
        assert!(!f.path_exists(STATE_DIRNAME));
    }

    #[test]
    fn is_a_no_op_the_second_time_and_on_a_project_ambit_never_touched() {
        let f = installed();
        f.cli(&["clean"]);
        let before = f.snapshot();

        let second = f.cli(&["clean"]);

        assert_eq!(second.code, ExitCode::Success, "{}", second.stderr);
        assert_eq!(f.snapshot(), before);
        assert_eq!(
            second.stdout,
            "removed (0)\n  (none)\n\nrecords (0)\n  (none)\n"
        );
    }

    #[test]
    fn lists_what_it_removed_artifacts_and_records_apart() {
        let f = installed();
        let result = f.cli(&["clean"]);

        let width = format!("{SKILLS_DIR}/{CORE_SKILL}").len();
        let pad = |text: &str| pad_end(text, width);

        assert_eq!(
            result.stdout,
            lines_out(&[
                "removed (7)".to_owned(),
                format!("  {}  hook-dir        -", pad(SCRIPT_HOOK_DIR)),
                format!(
                    "  {}  skill-dir       -",
                    pad(&format!("{SKILLS_DIR}/{ENGINEERING_SKILL}"))
                ),
                format!(
                    "  {}  skill-dir       -",
                    pad(&format!("{SKILLS_DIR}/{CORE_SKILL}"))
                ),
                format!(
                    "  {}  skill-dir       -",
                    pad(&format!("{SKILLS_DIR}/{FRONTEND_SKILL}"))
                ),
                format!(
                    "  {}  harness-config  {}, {}",
                    pad(CLAUDE_SETTINGS),
                    engineering_hook_key(),
                    core_hook_key()
                ),
                format!("  {}  skills-link     -", pad(CLAUDE_LINK)),
                format!(
                    "  {}  harness-config  mcpServers.{PACKED_MCP}",
                    pad(MCP_FILE)
                ),
                String::new(),
                "records (3)".to_owned(),
                format!("  {}", state_file()),
                format!("  {GITIGNORE_FILENAME} (managed block)"),
                format!("  {SHARED_GITIGNORE_FILE} (managed block)"),
            ])
        );
    }

    fn expected_removal() -> Value {
        json!({
            "gitignoreRemoved": [GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE],
            "removed": [
                { "kind": "hook-dir", "path": SCRIPT_HOOK_DIR },
                { "kind": "skill-dir", "path": format!("{SKILLS_DIR}/{ENGINEERING_SKILL}") },
                { "kind": "skill-dir", "path": format!("{SKILLS_DIR}/{CORE_SKILL}") },
                { "kind": "skill-dir", "path": format!("{SKILLS_DIR}/{FRONTEND_SKILL}") },
                {
                    "kind": "harness-config",
                    "managedKeys": [engineering_hook_key(), core_hook_key()],
                    "path": CLAUDE_SETTINGS,
                },
                { "kind": "skills-link", "path": CLAUDE_LINK },
                {
                    "kind": "harness-config",
                    "managedKeys": [format!("mcpServers.{PACKED_MCP}")],
                    "path": MCP_FILE,
                },
            ],
            "stateRemoved": true,
        })
    }

    #[test]
    fn emits_machine_readable_output_carrying_no_absolute_paths() {
        let f = installed();
        let result = f.cli(&["clean", "--json"]);
        let report: Value = serde_json::from_str(&result.stdout).unwrap();

        assert_eq!(report, expected_removal());
        assert!(!result.stdout.contains(&*f.root.to_string_lossy()));
    }

    #[test]
    fn reports_what_it_would_remove_under_dry_run_and_removes_none_of_it() {
        let f = installed();
        let before = f.snapshot();

        let result = f.cli(&["clean", "--dry-run", "--json"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        let report: Value = serde_json::from_str(&result.stdout).unwrap();

        assert_eq!(report, expected_removal());
        assert_eq!(f.snapshot(), before);
        assert_eq!(
            f.installed_skills(),
            sorted(&[ENGINEERING_SKILL, CORE_SKILL, FRONTEND_SKILL])
        );
    }

    #[test]
    fn unlinks_a_linked_skill_without_following_it_into_the_catalog() {
        let f = installed();
        clean_project(&f.project_dir, CleanOptions::default()).unwrap();

        assert_eq!(f.installed_skills().len(), 0);
        assert!(
            read_text(&f.catalog_dir.join("skills/company-context/SKILL.md"))
                .unwrap()
                .contains(CORE_SKILL)
        );
    }

    #[test]
    fn leaves_a_project_reinstallable_with_no_ownership_conflict_to_adopt_past() {
        let f = installed();
        f.cli(&["clean"]);

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(
            f.installed_skills(),
            sorted(&[ENGINEERING_SKILL, CORE_SKILL, FRONTEND_SKILL])
        );
    }
}

mod operation_lock {
    use super::*;
    use crate::project::operation_lock::{OPERATION_IN_PROGRESS, SetupLock};

    #[test]
    fn prune_and_clean_refuse_while_another_operation_holds_the_project() {
        let f = installed();
        f.write_profile(&["core"]);
        let held = SetupLock::acquire(&f.project_dir).unwrap();
        let before = f.snapshot();

        for command in ["prune", "clean"] {
            let result = f.cli(&[command]);

            assert_eq!(
                result.code,
                ExitCode::Config,
                "{command}: {}",
                result.stderr
            );
            assert!(
                result.stderr.contains(OPERATION_IN_PROGRESS),
                "{command}: {}",
                result.stderr
            );
        }

        assert_eq!(f.snapshot(), before);

        drop(held);

        assert_eq!(f.cli(&["prune"]).code, ExitCode::Success);
        assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);
        assert!(!f.path_exists(STATE_DIRNAME));
    }

    #[test]
    fn a_prune_with_nothing_to_do_takes_no_lock_and_writes_nothing() {
        let f = installed();
        let _held = SetupLock::acquire(&f.project_dir).unwrap();
        let before = f.snapshot();

        assert_eq!(f.cli(&["prune"]).code, ExitCode::Success);
        assert_eq!(f.snapshot(), before);
    }
}
