//! `ambit status`: does the project match what resolution now produces?
//!
//! The interesting assertions are the negative ones. A status command is only worth running if it
//! is quiet about everything ambit does not own (a hand-added server in `.mcp.json`, a hand-written
//! skill directory beside ambit's), so every drift case here also pins what is *not* reported.
//!
//! `--check` is asserted in both directions every time: exit 5 on drift and 0 when clean. A checker
//! that always failed and a checker that never did would each satisfy half of it.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;
use crate::errors::ExitCode;
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::{CliResult, run_cli, tempdir, test_env};
use crate::util::fs::read_dir_names;
use crate::util::json::{JsonValue, parse, stringify_pretty};
use crate::util::text::pad_end;

const CATALOG_NAME: &str = "company";
const SKILLS_DIR: &str = ".agents/skills";
const CLAUDE_LINK: &str = ".claude/skills";
const MCP_FILE: &str = ".mcp.json";

const CORE_SKILL: &str = "company-context";
const ENGINEERING_SKILL: &str = "code-review";
const FRONTEND_SKILL: &str = "design-tokens";

/// The fixture's tag-matched http server, and the one only `requires` reaches.
const PACKED_MCP: &str = "linter";
const FIXTURE_MCP: &str = "fixture";

/// The fixture's script-shipping hook and the file both its hooks' entries land in.
///
/// The default profile holds `core` and `function.engineering`, which select one inline-command
/// hook and one shipping a script, so every row list here carries a `hook-dir` and a second config
/// file besides the skills.
const HOOK_TARGET: &str = ".agents/hooks/guard-secrets";
const CLAUDE_SETTINGS: &str = ".claude/settings.json";

fn core_target() -> String {
    format!("{SKILLS_DIR}/{CORE_SKILL}")
}

fn frontend_target() -> String {
    format!("{SKILLS_DIR}/{FRONTEND_SKILL}")
}

fn engineering_target() -> String {
    format!("{SKILLS_DIR}/{ENGINEERING_SKILL}")
}

/// A fixture catalog beside a project that points at it.
///
/// The environment leaves `LINTER_API_KEY` unset: the tagged server interpolates it into a header,
/// so what is on disk depends on it.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    catalog_dir: PathBuf,
    project_dir: PathBuf,
    env: Env,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempdir();
        let root = dir.path().to_path_buf();
        let catalog_dir = root.join("catalog");
        let project_dir = root.join("project");

        build_fixture_catalog(&catalog_dir).expect("build the fixture catalog");
        fs::create_dir_all(&project_dir).unwrap();

        let fixture = Self {
            env: test_env(&root),
            _dir: dir,
            root,
            catalog_dir,
            project_dir,
        };

        // Three skills (`function.engineering` also selects its nested frontend child) plus the
        // `tagged` http server, which declares that same tag.
        fixture.write_profile(&["core", "function.engineering", "function.engineering.*"]);
        fixture
    }

    /// Points the project at the fixture catalog and gives it a `requires` list.
    fn write_profile(&self, packs: &[&str]) {
        let list = if packs.is_empty() {
            "[]".to_owned()
        } else {
            let entries: Vec<String> = packs
                .iter()
                .map(|pack| format!("  - {{ pack: \"{CATALOG_NAME}/{pack}\" }}"))
                .collect();
            format!("\n{}", entries.join("\n"))
        };

        fs::write(
            self.project_dir.join("ambit.yml"),
            format!(
                "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nrequires: {list}\n"
            ),
        )
        .unwrap();
    }

    /// Runs the CLI against the project. Lines are joined with `\n` and the final newline
    /// dropped.
    fn cli(&self, args: &[&str]) -> CliResult {
        let project = self.project_dir.to_string_lossy().into_owned();
        let mut argv: Vec<&str> = args.to_vec();
        argv.extend(["--project", &project]);

        let mut result = run_cli(&argv, &self.root, &self.env);
        result.stdout = trim_newline(result.stdout);
        result.stderr = trim_newline(result.stderr);
        result
    }

    fn status(&self) -> ProjectStatus {
        project_status(&self.project_dir, &self.env, StatusOptions::default())
            .expect("status succeeds")
    }

    /// Every artifact status reports, as `path=state` pairs, so a whole report fits one assertion.
    fn states(&self) -> Vec<String> {
        self.status()
            .artifacts
            .iter()
            .map(|artifact| format!("{}={}", artifact.path, artifact.state))
            .collect()
    }

    /// The detail line status gives for one path.
    fn detail_of(&self, target: &str) -> Option<String> {
        self.status()
            .artifacts
            .into_iter()
            .find(|artifact| artifact.path == target)
            .map(|artifact| artifact.detail)
    }

    fn write_mcp_file(&self, document: &JsonValue) {
        fs::write(
            self.project_dir.join(MCP_FILE),
            format!("{}\n", stringify_pretty(document)),
        )
        .unwrap();
    }

    fn read_mcp_config(&self) -> JsonValue {
        parse(&crate::util::fs::read_text(&self.project_dir.join(MCP_FILE)).unwrap()).unwrap()
    }

    /// Every file in the project, keyed by relative path and carrying its contents.
    ///
    /// Symlinks are followed, because the default install of a `path:` catalog is a link and the
    /// claim being made is about the files a harness would read.
    fn snapshot(&self) -> BTreeMap<String, String> {
        fn walk(current: &Path, relative: &str, found: &mut BTreeMap<String, String>) {
            for name in read_dir_names(current).unwrap() {
                let within = if relative.is_empty() {
                    name.clone()
                } else {
                    format!("{relative}/{name}")
                };
                let absolute = current.join(&name);

                if fs::metadata(&absolute).unwrap().is_dir() {
                    walk(&absolute, &within, found);
                } else {
                    found.insert(within, crate::util::fs::read_text(&absolute).unwrap());
                }
            }
        }

        let mut found = BTreeMap::new();
        walk(&self.project_dir, "", &mut found);
        found
    }
}

fn trim_newline(mut text: String) -> String {
    if text.ends_with('\n') {
        text.pop();
    }

    text
}

fn all_with(states: [&str; 7]) -> Vec<String> {
    [
        HOOK_TARGET.to_owned(),
        engineering_target(),
        core_target(),
        frontend_target(),
        CLAUDE_SETTINGS.to_owned(),
        CLAUDE_LINK.to_owned(),
        MCP_FILE.to_owned(),
    ]
    .iter()
    .zip(states)
    .map(|(path, state)| format!("{path}={state}"))
    .collect()
}

fn installed(args: &[&str]) -> Fixture {
    let fixture = Fixture::new();
    let result = fixture.cli(args);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    fixture
}

fn row(path: &str, state: ArtifactState) -> StatusArtifact {
    StatusArtifact {
        path: path.to_owned(),
        kind: ArtifactKind::SkillDir,
        state,
        detail: String::new(),
    }
}

// The pure projections over a status, which need nothing resolved.

#[test]
fn drift_is_every_row_that_is_not_ok() {
    let status = ProjectStatus {
        artifacts: vec![
            row("a", ArtifactState::Ok),
            row("b", ArtifactState::Missing),
            row("c", ArtifactState::Stale),
        ],
    };

    assert_eq!(
        status_drift(&status),
        [
            row("b", ArtifactState::Missing),
            row("c", ArtifactState::Stale)
        ]
    );
    assert!(!is_clean(&status));
}

#[test]
fn a_status_with_nothing_drifted_is_clean() {
    let status = ProjectStatus {
        artifacts: vec![row("a", ArtifactState::Ok)],
    };

    assert_eq!(status_drift(&status), []);
    assert!(is_clean(&status));
    assert!(is_clean(&ProjectStatus::default()));
}

#[test]
fn an_empty_plan_against_empty_state_reports_nothing() {
    let status = status_of_plan(&[], &State::empty()).unwrap();

    assert_eq!(status, ProjectStatus::default());
}

// ambit status on an installed project

#[test]
fn reports_every_artifact_as_matching_and_says_so_rather_than_printing_nothing() {
    let fixture = installed(&["install"]);
    let result = fixture.cli(&["status"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

    // The detail column is empty on every row here, so `columns` trims it away and the state ends
    // each line, but the two columns before it are still padded out to their widest cell.
    let width = core_target().len();
    let kind = "harness-config".len();

    assert_eq!(
        result.stdout,
        [
            "artifacts (7)".to_owned(),
            format!(
                "  {}  {}  ok",
                pad_end(HOOK_TARGET, width),
                pad_end("hook-dir", kind)
            ),
            format!(
                "  {}  {}  ok",
                pad_end(&engineering_target(), width),
                pad_end("skill-dir", kind)
            ),
            format!("  {}  {}  ok", core_target(), pad_end("skill-dir", kind)),
            format!(
                "  {}  {}  ok",
                pad_end(&frontend_target(), width),
                pad_end("skill-dir", kind)
            ),
            format!("  {}  harness-config  ok", pad_end(CLAUDE_SETTINGS, width)),
            format!(
                "  {}  {}  ok",
                pad_end(CLAUDE_LINK, width),
                pad_end("skills-link", kind)
            ),
            format!("  {}  harness-config  ok", pad_end(MCP_FILE, width)),
        ]
        .join("\n")
    );
}

#[test]
fn exits_0_under_check_when_nothing_has_drifted() {
    let fixture = installed(&["install"]);
    let result = fixture.cli(&["status", "--check"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
}

#[test]
fn touches_nothing_so_it_can_be_run_on_a_project_it_would_report_drift_on() {
    let fixture = installed(&["install"]);
    let before = fixture.snapshot();

    assert_eq!(fixture.cli(&["status"]).code, ExitCode::Success);
    assert_eq!(fixture.cli(&["status", "--check"]).code, ExitCode::Success);

    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn emits_machine_readable_output_carrying_no_absolute_paths() {
    let fixture = installed(&["install"]);
    let result = fixture.cli(&["status", "--json"]);

    assert_eq!(
        parse(&result.stdout).unwrap(),
        json!({
            "artifacts": [
                { "kind": "hook-dir", "path": HOOK_TARGET, "state": "ok" },
                { "kind": "skill-dir", "path": engineering_target(), "state": "ok" },
                { "kind": "skill-dir", "path": core_target(), "state": "ok" },
                { "kind": "skill-dir", "path": frontend_target(), "state": "ok" },
                { "kind": "harness-config", "path": CLAUDE_SETTINGS, "state": "ok" },
                { "kind": "skills-link", "path": CLAUDE_LINK, "state": "ok" },
                { "kind": "harness-config", "path": MCP_FILE, "state": "ok" },
            ],
            "clean": true,
        })
    );
    assert!(!result.stdout.contains(&*fixture.root.to_string_lossy()));
}

// ambit status after a manual edit.
//
// Content drift is a question about a *copy*: a symlinked skill has no bytes of its own, so these
// cases install with `--copy`. Editing a linked skill edits the catalog, and the symlinked block
// below pins that as the non-drift it is.

#[test]
fn reports_an_edited_skill_file_as_modified_naming_the_file() {
    let fixture = installed(&["install", "--copy"]);
    fs::write(
        fixture.project_dir.join(core_target()).join("SKILL.md"),
        "---\nname: edited by hand\n---\n",
    )
    .unwrap();

    let result = fixture.cli(&["status"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert!(
        result
            .stdout
            .contains("modified  SKILL.md differs from its source")
    );
    assert_eq!(
        fixture.states(),
        all_with(["ok", "ok", "modified", "ok", "ok", "ok", "ok"])
    );
}

#[test]
fn exits_5_under_check_once_a_skill_has_been_edited() {
    let fixture = installed(&["install", "--copy"]);
    fs::write(
        fixture.project_dir.join(core_target()).join("SKILL.md"),
        "edited\n",
    )
    .unwrap();

    let result = fixture.cli(&["status", "--check"]);

    assert_eq!(result.code, ExitCode::Drift);
    // The report is still the report: `--check` adds an exit code, not an error.
    assert_eq!(result.stderr, "");
    assert!(result.stdout.contains("modified"));
}

#[test]
fn reports_a_file_added_into_an_installed_skill_which_install_would_remove() {
    let fixture = installed(&["install", "--copy"]);
    fs::write(
        fixture.project_dir.join(core_target()).join("notes.md"),
        "mine\n",
    )
    .unwrap();

    assert_eq!(
        fixture.detail_of(&core_target()).as_deref(),
        Some("notes.md is not in its source")
    );
}

#[test]
fn reports_a_deleted_skill_directory_as_missing() {
    let fixture = installed(&["install", "--copy"]);
    fs::remove_dir_all(fixture.project_dir.join(engineering_target())).unwrap();

    assert_eq!(
        fixture.states(),
        all_with(["ok", "missing", "ok", "ok", "ok", "ok", "ok"])
    );
    assert_eq!(
        fixture.detail_of(&engineering_target()).as_deref(),
        Some("nothing is installed at this path")
    );
}

#[test]
fn reports_an_edited_server_as_modified_naming_the_key() {
    let fixture = installed(&["install", "--copy"]);
    fixture.write_mcp_file(&json!({ "mcpServers": { PACKED_MCP: { "command": "my-own-thing" } } }));

    assert_eq!(
        fixture.detail_of(MCP_FILE),
        Some(format!(
            "\"mcpServers.{PACKED_MCP}\" is not what install would write"
        ))
    );
    assert!(fixture.states().contains(&format!("{MCP_FILE}=modified")));
}

#[test]
fn reports_a_deleted_server_as_absent_rather_than_as_modified() {
    let fixture = installed(&["install", "--copy"]);
    fixture.write_mcp_file(&json!({ "mcpServers": {} }));

    assert_eq!(
        fixture.detail_of(MCP_FILE),
        Some(format!("\"mcpServers.{PACKED_MCP}\" is absent"))
    );
    assert!(fixture.states().contains(&format!("{MCP_FILE}=missing")));
}

#[test]
fn reports_a_mcp_json_deleted_outright_since_install_would_write_it_again() {
    let fixture = installed(&["install", "--copy"]);
    fs::remove_file(fixture.project_dir.join(MCP_FILE)).unwrap();

    assert!(fixture.states().contains(&format!("{MCP_FILE}=missing")));
}

#[test]
fn does_not_read_a_reordered_server_as_drift_ambit_owns_the_key_not_the_layout() {
    let fixture = installed(&["install", "--copy"]);
    let document = fixture.read_mcp_config();
    let tagged = document["mcpServers"][PACKED_MCP].clone();

    fixture.write_mcp_file(&json!({
        "mcpServers": {
            PACKED_MCP: { "headers": tagged["headers"], "url": tagged["url"], "type": tagged["type"] }
        }
    }));

    assert!(is_clean(&fixture.status()));
}

#[test]
fn says_nothing_about_a_hand_added_server_or_any_other_key_in_the_file() {
    let fixture = installed(&["install", "--copy"]);
    let mut document = fixture.read_mcp_config();
    document["mcpServers"]["handmade"] = json!({ "command": "node" });
    document["extra"] = json!({ "kept": true });

    fixture.write_mcp_file(&document);

    assert_eq!(
        fixture.states(),
        all_with(["ok", "ok", "ok", "ok", "ok", "ok", "ok"])
    );
}

#[test]
fn says_nothing_about_a_skill_directory_no_state_claims() {
    let fixture = installed(&["install", "--copy"]);
    let target = fixture.project_dir.join(SKILLS_DIR).join("hand-written");

    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), "---\nname: hand-written\n---\n").unwrap();

    assert!(is_clean(&fixture.status()));
}

#[test]
fn reports_a_change_in_the_catalog_not_only_one_in_the_project() {
    let fixture = installed(&["install", "--copy"]);
    fs::write(
        fixture.catalog_dir.join("skills/company-context/SKILL.md"),
        "---\nname: company-context\n---\n\n# rewritten upstream\n",
    )
    .unwrap();

    assert_eq!(
        fixture.detail_of(&core_target()).as_deref(),
        Some("SKILL.md differs from its source")
    );
}

// ambit status on a symlinked install.
//
// The other half of the materialization modes: what is on disk decides how a skill is compared, so
// a link is checked for pointing at its source and a copy for holding its bytes.

const CORE_SOURCE: &str = "skills/company-context";

#[test]
fn says_nothing_when_the_source_is_edited_through_the_link_which_is_what_linking_is_for() {
    let fixture = installed(&["install"]);

    fs::write(
        fixture.project_dir.join(core_target()).join("SKILL.md"),
        "---\nname: company-context\n---\n\n# edited in place\n",
    )
    .unwrap();

    // The edit landed in the catalog, so there is no second copy for the two to disagree about.
    assert!(
        crate::util::fs::read_text(&fixture.catalog_dir.join(CORE_SOURCE).join("SKILL.md"))
            .unwrap()
            .contains("edited in place")
    );
    assert!(is_clean(&fixture.status()));
    assert_eq!(fixture.cli(&["status", "--check"]).code, ExitCode::Success);
}

#[cfg(unix)]
#[test]
fn reports_a_link_pointing_elsewhere_as_modified_naming_where_it_points() {
    let fixture = installed(&["install"]);
    let target = fixture.project_dir.join(core_target());

    crate::util::fs::rm_rf(&target).unwrap();
    crate::util::fs::symlink_dir(Path::new("../../../elsewhere"), &target).unwrap();

    // A link is not followed, so one pointing at nothing is drift rather than an absent artifact.
    assert_eq!(
        fixture.detail_of(&core_target()).as_deref(),
        Some("it points at ../../../elsewhere, not at its source")
    );
    assert!(
        fixture
            .states()
            .contains(&format!("{}=modified", core_target()))
    );
    assert_eq!(fixture.cli(&["status", "--check"]).code, ExitCode::Drift);
}

#[test]
fn reports_a_file_sitting_where_a_linked_skill_belongs_as_modified() {
    let fixture = installed(&["install"]);
    let target = fixture.project_dir.join(core_target());

    crate::util::fs::rm_rf(&target).unwrap();
    fs::write(&target, "not a skill directory\n").unwrap();

    assert_eq!(
        fixture.detail_of(&core_target()).as_deref(),
        Some("it is not a directory")
    );
}

#[test]
fn reads_an_intact_copy_as_clean_even_though_a_plain_install_would_relink_it() {
    let fixture = installed(&["install", "--copy"]);

    // Mode is a per-run choice and both modes put the same bytes in front of the harness, so
    // `--copy` must not leave `status --check` permanently red.
    assert_eq!(
        fixture.states(),
        all_with(["ok", "ok", "ok", "ok", "ok", "ok", "ok"])
    );
    assert_eq!(fixture.cli(&["status", "--check"]).code, ExitCode::Success);
}

// ambit status before an install

#[test]
fn reports_every_artifact_resolution_wants_as_missing() {
    let fixture = Fixture::new();
    let result = fixture.cli(&["status"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        fixture.states(),
        all_with([
            "missing", "missing", "missing", "missing", "missing", "missing", "missing"
        ])
    );
    assert_eq!(fixture.cli(&["status", "--check"]).code, ExitCode::Drift);
}

#[test]
fn reports_a_target_install_would_refuse_as_unowned_rather_than_as_modified() {
    let fixture = Fixture::new();
    let target = fixture.project_dir.join(core_target());

    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), "---\nname: not ambit's\n---\n").unwrap();
    fixture.write_mcp_file(
        &json!({ "mcpServers": { PACKED_MCP: { "command": "not ambit's either" } } }),
    );

    // Nothing is installed here, so the link is absent like the skills it would point at.
    assert_eq!(
        fixture.states(),
        all_with([
            "missing", "missing", "unowned", "missing", "missing", "missing", "unowned"
        ])
    );
    assert_eq!(
        fixture.detail_of(&core_target()).as_deref(),
        Some("it exists but ambit did not create it")
    );
    assert_eq!(
        fixture.detail_of(MCP_FILE),
        Some(format!(
            "\"mcpServers.{PACKED_MCP}\" exists but ambit did not create it"
        ))
    );
}

#[test]
fn reports_nothing_at_all_for_a_project_that_resolves_to_nothing() {
    let fixture = Fixture::new();
    fixture.write_profile(&[]);

    let result = fixture.cli(&["status"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(result.stdout, ["artifacts (0)", "  (none)"].join("\n"));
    assert_eq!(fixture.cli(&["status", "--check"]).code, ExitCode::Success);
}

// ambit status after the profile narrows: what install would prune, before it prunes it.

#[test]
fn reports_what_ambit_owns_and_nothing_selects_as_stale() {
    let fixture = installed(&["install"]);
    fixture.write_profile(&["core"]);

    // The settings file is stale for the same reason `.mcp.json` is: one key it holds is no longer
    // selected, even though the entry the narrowed profile's own hook wrote still matches.
    assert_eq!(
        fixture.states(),
        all_with(["stale", "stale", "ok", "stale", "stale", "ok", "stale"])
    );
    assert_eq!(
        fixture.detail_of(&frontend_target()).as_deref(),
        Some("ambit owns it, and nothing selects it now")
    );
    assert_eq!(fixture.cli(&["status", "--check"]).code, ExitCode::Drift);
}

#[test]
fn reports_a_single_stale_server_key_in_a_file_whose_other_keys_still_match() {
    let fixture = Fixture::new();
    // Both servers: `tagged` by tag, `fixture` through the project skill's `requires`.
    fixture.write_profile(&[
        "function.engineering",
        "function.engineering.*",
        "project.acme",
    ]);
    assert_eq!(fixture.cli(&["install"]).code, ExitCode::Success);
    fixture.write_profile(&["function.engineering", "function.engineering.*"]);

    assert_eq!(
        fixture.detail_of(MCP_FILE),
        Some(format!(
            "\"mcpServers.{FIXTURE_MCP}\" is no longer selected"
        ))
    );
    assert!(fixture.states().contains(&format!("{MCP_FILE}=stale")));
}

#[test]
fn goes_quiet_again_once_the_install_that_prunes_them_has_run() {
    let fixture = installed(&["install"]);
    fixture.write_profile(&["core"]);
    assert_eq!(fixture.cli(&["install"]).code, ExitCode::Success);

    let status = fixture.status();

    assert_eq!(status_drift(&status), []);
    assert_eq!(fixture.cli(&["status", "--check"]).code, ExitCode::Success);
}

// Copies with symlinks in them, compared by the digest state recorded at install.

/// The status row for one path.
fn row_at(fixture: &Fixture, path: &str) -> StatusArtifact {
    fixture
        .status()
        .artifacts
        .into_iter()
        .find(|artifact| artifact.path == path)
        .unwrap_or_else(|| panic!("no row for {path}"))
}

#[cfg(unix)]
#[test]
fn reads_a_fresh_copy_holding_links_inside_and_outside_its_tree_as_ok() {
    use crate::util::fs::symlink_file;

    let fixture = Fixture::new();
    let skill = fixture.catalog_dir.join("skills").join(ENGINEERING_SKILL);

    symlink_file(Path::new("SKILL.md"), &skill.join("alias.md")).unwrap();
    symlink_file(
        Path::new("../../packs/core.yml"),
        &skill.join("outside.yml"),
    )
    .unwrap();

    let install = fixture.cli(&["install", "--copy"]);

    assert_eq!(install.code, ExitCode::Success, "{}", install.stderr);
    assert_eq!(
        row_at(&fixture, &engineering_target()).state,
        ArtifactState::Ok
    );
    assert_eq!(fixture.cli(&["status", "--check"]).code, ExitCode::Success);
}

#[cfg(unix)]
#[test]
fn reports_a_copy_edited_in_a_way_no_file_comparison_shows() {
    use crate::util::fs::symlink_file;

    let fixture = Fixture::new();
    let skill = fixture.catalog_dir.join("skills").join(ENGINEERING_SKILL);

    symlink_file(Path::new("SKILL.md"), &skill.join("alias.md")).unwrap();
    assert_eq!(fixture.cli(&["install", "--copy"]).code, ExitCode::Success);

    // Same bytes through the link, different link: only the digest can tell.
    let alias = fixture
        .project_dir
        .join(engineering_target())
        .join("alias.md");

    fs::remove_file(&alias).unwrap();
    symlink_file(Path::new("./SKILL.md"), &alias).unwrap();

    let row = row_at(&fixture, &engineering_target());

    assert_eq!(row.state, ArtifactState::Modified);
    assert_eq!(row.detail, "its contents changed since ambit installed it");
}
