//! More than one harness in one project.
//!
//! The single-harness path is covered exhaustively in `tests.rs`; what only a multi-harness install
//! can show is what the shared skills directory implies. Two harnesses of the same family name the
//! same skills link, and *every* harness names the same skill directories, so the interesting
//! claims are about a target two adapters both want, and about each harness's own config file
//! being written in its own format at the same time.
//!
//! The migration cases belong here for the same reason: the layout they migrate from is the one
//! that existed before the skills directory was shared.
//!
//! Every case installs a real project (B1, B2, B3); the `status` and `doctor` cases need B5 and B2
//! as well.

use pretty_assertions::assert_eq;
use serde_json::json;

use super::fixture::*;
use super::*;
use crate::errors::ExitCode;
use crate::model::documents::{DocumentFormat, DocumentShape};
use crate::model::state::{ArtifactKind, STATE_DIRNAME, STATE_FILENAME};
use crate::util::fs;
use crate::util::path::relative;

const ALL_SKILLS: &[&str] = &[ENGINEERING_SKILL, CORE_SKILL, FRONTEND_SKILL];
const SHARED_HOOKS_DIR: &str = ".agents/hooks";

fn all_skills_sorted() -> Vec<String> {
    let mut skills = strings(ALL_SKILLS);

    skills.sort();
    skills
}

fn skills_listed(project: &Project) -> Vec<String> {
    let mut names = fs::read_dir_names(&project.path(SKILLS_DIR)).expect("a skills directory");

    names.sort();
    names
}

fn readlink(project: &Project, relative: &str) -> String {
    project.link_at(relative).expect("a symlink")
}

/// The default packs, for the given harnesses.
fn write_harnesses(project: &Project, harnesses: &[&str]) {
    project.write(
        "ambit.yml",
        &format!(
            "version: 1\nharnesses: [{}]\ncatalogs:\n  - name: company\n    source: path:../catalog\nrequires:\n{}\n{}\n{}\n",
            harnesses.join(", "),
            requires_entry("core"),
            requires_entry("function.engineering"),
            requires_entry("function.engineering.*"),
        ),
    );
}

fn with_harnesses(harnesses: &[&str]) -> Project {
    let project = Project::new();

    write_harnesses(&project, harnesses);
    project
}

fn parsed(project: &Project, relative: &str) -> serde_json::Value {
    serde_json::from_str(&project.read(relative)).expect("valid JSON")
}

fn server_names(project: &Project, relative: &str) -> Vec<String> {
    parsed(project, relative)["mcpServers"]
        .as_object()
        .expect("a servers section")
        .keys()
        .cloned()
        .collect()
}

// two harnesses of the same family
//
// Claude Code and Cursor both read `.claude/skills`, so exactly one link is planned, applied, owned
// and ignored: not two, and not one applied twice, which would mean the second adapter unlinking
// what the first had just created.

#[test]
#[ignore = "needs B1, B2, B3"]
fn plans_the_shared_link_and_the_shared_skill_directories_once_each() {
    let project = with_harnesses(&["claude", "cursor"]);
    let result = install_project(&project.dir, &project.env, InstallOptions::default(), &[])
        .expect("an install");

    assert_eq!(result.harnesses, ["claude", "cursor"]);
    // One hook directory, shared like the skills, and a config file per harness, because the two
    // read their hooks from files of their own even where they share a skills link.
    assert_eq!(
        result
            .artifacts
            .iter()
            .map(|artifact| artifact.path.clone())
            .collect::<Vec<_>>(),
        [
            format!("{SKILLS_DIR}/{ENGINEERING_SKILL}"),
            format!("{SKILLS_DIR}/{CORE_SKILL}"),
            format!("{SKILLS_DIR}/{FRONTEND_SKILL}"),
            HOOK_DIR.to_owned(),
            CLAUDE_LINK.to_owned(),
            ".mcp.json".to_owned(),
            ".claude/settings.json".to_owned(),
            ".cursor/mcp.json".to_owned(),
            ".cursor/hooks.json".to_owned(),
        ]
    );
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn writes_both_config_files_and_one_skills_tree() {
    let project = with_harnesses(&["claude", "cursor"]);
    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(server_names(&project, ".mcp.json"), [PACKED_MCP]);
    assert_eq!(server_names(&project, ".cursor/mcp.json"), [PACKED_MCP]);
    assert_eq!(skills_listed(&project), all_skills_sorted());
    // One link, and it points at the shared directory rather than at a copy of it.
    assert_eq!(readlink(&project, CLAUDE_LINK), format!("../{SKILLS_DIR}"));
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn records_each_shared_artifact_once() {
    let project = with_harnesses(&["claude", "cursor"]);

    project.cli(&["install"]);

    let paths: Vec<String> = project
        .state_artifacts()
        .into_iter()
        .map(|artifact| artifact.path)
        .collect();
    let unique: indexmap::IndexSet<&String> = paths.iter().collect();

    assert_eq!(unique.len(), paths.len());
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn lists_the_link_once_in_the_managed_gitignore_block() {
    let project = with_harnesses(&["claude", "cursor"]);

    project.cli(&["install"]);

    assert_eq!(project.read(".gitignore").matches(CLAUDE_LINK).count(), 1);
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn changes_no_bytes_on_a_second_install() {
    let project = with_harnesses(&["claude", "cursor"]);

    project.cli(&["install"]);
    let files = [".mcp.json", ".cursor/mcp.json", ".gitignore"];
    let before: Vec<String> = files.iter().map(|file| project.read(file)).collect();

    let second = project.cli(&["install"]);

    assert_eq!(second.code, ExitCode::Success, "{}", second.stderr);
    assert_eq!(
        files
            .iter()
            .map(|file| project.read(file))
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(readlink(&project, CLAUDE_LINK), format!("../{SKILLS_DIR}"));
}

// two harnesses from different families
//
// The case that exercises two document formats in one run: `.mcp.json` through the JSON driver and
// `.codex/config.toml` through the TOML one.

#[test]
#[ignore = "needs B1, B2, B3"]
fn writes_each_harnesss_config_in_that_harnesss_own_file_and_format() {
    let project = with_harnesses(&["claude", "codex"]);
    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        parsed(&project, ".mcp.json"),
        json!({
            "mcpServers": {
                PACKED_MCP: {
                    "type": "http",
                    "url": "https://mcp.invalid/fixture",
                    "headers": { "Authorization": format!("Bearer ${{{PACKED_KEY_VAR}}}") },
                },
            },
        })
    );
    assert_eq!(
        project.read(".codex/config.toml"),
        format!(
            "[mcp_servers.linter]\nurl = \"https://mcp.invalid/fixture\"\nbearer_token_env_var = \"{PACKED_KEY_VAR}\"\n"
        )
    );
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn materializes_the_skills_once_and_links_only_for_the_harness_that_needs_it() {
    let project = with_harnesses(&["claude", "codex"]);

    project.cli(&["install"]);

    assert_eq!(skills_listed(&project), all_skills_sorted());
    // Codex reads `.agents/skills` natively, so it gets no directory of its own.
    assert!(project.lexists(CLAUDE_LINK));
    assert!(!project.lexists(".codex/skills"));
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn records_the_format_of_each_config_file_so_pruning_knows_how_to_edit_it() {
    let project = with_harnesses(&["claude", "codex"]);

    project.cli(&["install"]);

    assert_eq!(
        project
            .state_artifacts()
            .into_iter()
            .filter(|artifact| artifact.kind == ArtifactKind::HarnessConfig)
            .collect::<Vec<_>>(),
        // In state's own order, which is by path, so a diff of two installs reads as a diff.
        [
            config(
                ".claude/settings.json",
                DocumentFormat::Json,
                Some(DocumentShape::Array),
                hook_keys(CLAUDE_HOOK_ROOT),
            ),
            config(
                ".codex/config.toml",
                DocumentFormat::Toml,
                None,
                vec![format!("mcp_servers.{PACKED_MCP}")],
            ),
            // The same entry shape, and one string apart: Codex interpolates nothing, so its copy
            // of the script-shipping hook carries the project-relative path.
            config(
                ".codex/hooks.json",
                DocumentFormat::Json,
                Some(DocumentShape::Array),
                hook_keys(SHARED_HOOKS_DIR),
            ),
            config(
                ".mcp.json",
                DocumentFormat::Json,
                None,
                vec![format!("mcpServers.{PACKED_MCP}")],
            ),
        ]
    );
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn prunes_the_stale_server_from_both_files_in_both_formats() {
    let project = with_harnesses(&["claude", "codex"]);

    project.cli(&["install"]);
    // A profile with no servers, so the one both files hold is stale.
    project.write(
        "ambit.yml",
        &format!(
            "version: 1\nharnesses: [claude, codex]\ncatalogs:\n  - name: company\n    source: path:../catalog\nrequires:\n{}\n",
            requires_entry("core")
        ),
    );

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(parsed(&project, ".mcp.json"), json!({ "mcpServers": {} }));
    // The TOML file is co-owned like the JSON one, so it stays, holding nothing of ambit's.
    assert_eq!(project.read(".codex/config.toml"), "");
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn leaves_a_codex_configs_own_settings_and_comments_exactly_as_they_were() {
    let project = with_harnesses(&["claude", "codex"]);
    let handwritten =
        "# Mine, not ambit's.\nmodel = \"gpt-5-codex\"\n\n[sandbox]\nmode = \"read-only\"\n";

    project.write(".codex/config.toml", handwritten);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert!(project.read(".codex/config.toml").starts_with(handwritten));
}

// all five harnesses at once

const ALL_HARNESSES: &[&str] = &["claude", "codex", "cursor", "opencode", "vscode"];

#[test]
#[ignore = "needs B1, B2, B3"]
fn writes_one_skills_tree_one_link_and_five_config_files() {
    let project = with_harnesses(ALL_HARNESSES);
    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(skills_listed(&project), all_skills_sorted());

    for file in [
        ".mcp.json",
        ".codex/config.toml",
        ".cursor/mcp.json",
        ".opencode/opencode.jsonc",
        ".vscode/mcp.json",
    ] {
        assert!(project.lexists(file), "{file}");
    }

    // Each skill is materialized once however many harnesses read it: three directories, not
    // fifteen.
    assert_eq!(
        project
            .state_artifacts()
            .iter()
            .filter(|artifact| artifact.kind == ArtifactKind::SkillDir)
            .count(),
        ALL_SKILLS.len()
    );
}

#[test]
#[ignore = "needs B1, B2, B3, B5"]
fn reports_no_drift_afterwards_in_all_three_document_formats_at_once() {
    let project = with_harnesses(ALL_HARNESSES);

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);

    let result = project.cli(&["status", "--check"]);

    // The one case that compares a JSON, a JSONC and a TOML file in a single run, each through its
    // own driver's notion of "already what install would write".
    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn passes_doctor_with_every_referenced_variable_set() {
    let mut project = with_harnesses(ALL_HARNESSES);

    project.cli(&["install"]);
    // Every variable the bundle references, whichever entity declared it: `doctor` reads the
    // environment rather than the files, so a reference in five formats is still one question.
    project
        .env
        .insert(PACKED_KEY_VAR.to_owned(), "s3cret".to_owned());
    project
        .env
        .insert("ACME_FIGMA_TOKEN".to_owned(), "figma-token".to_owned());

    let result = project.cli(&["doctor"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
}

// a variable referenced by a header and not declared in `env`
//
// The check is about the environment, so its answer must not depend on which harness's file the
// reference landed in, and every harness spells a reference differently, including one (Codex)
// that writes the bare variable name. A check that read the installed bytes would work for Claude
// Code and quietly stop working for the rest.

#[test]
#[ignore = "needs B1, B2, B3"]
fn is_reported_for_every_harness() {
    for harness in ALL_HARNESSES {
        let project = Project::new();

        project.write_catalog(
            "mcps/undeclared.yml",
            "name: undeclared\n\ntransport:\n  http:\n    url: https://mcp.invalid/undeclared\n    headers:\n      Authorization: \"Bearer ${UNDECLARED_TOKEN}\"\n",
        );
        // Beside the fixture's own `core` pack, rewritten so the profile's entry reaches this
        // server too.
        project.write_catalog(
            "packs/core.yml",
            &[
                "name: core",
                "description: What every Acme session needs, whoever is in it.",
                "requires:",
                "  - skill: company-context",
                "  - hook: session-notes",
                "  - mcp: undeclared",
                "",
            ]
            .join("\n"),
        );
        write_harnesses(&project, &[harness]);
        assert_eq!(
            project.cli(&["install"]).code,
            ExitCode::Success,
            "{harness}"
        );

        let result = project.cli(&["doctor"]);

        assert_eq!(result.code, ExitCode::Doctor, "{harness}");
        assert!(
            result
                .stdout
                .contains("unset environment variable \"UNDECLARED_TOKEN\""),
            "{harness}"
        );
        assert!(
            result
                .stdout
                .contains("references it, for the harness to expand at spawn"),
            "{harness}"
        );
    }
}

// migrating an old-layout .claude/skills
//
// The old layout put every skill in `.claude/skills/<name>/`, which is now the path of the link
// itself. ambit adopts that directory implicitly, but only when it installed everything in it,
// which is exactly when replacing it with a link loses nothing.

/// A `.claude/skills` holding skill directories a prior install recorded as ambit's.
fn write_old_layout(project: &Project, extra: Option<&str>) {
    let artifacts: Vec<serde_json::Value> = ALL_SKILLS
        .iter()
        .map(|name| json!({ "path": format!("{CLAUDE_LINK}/{name}"), "kind": "skill-dir", "mode": "link" }))
        .collect();
    let state = json!({ "version": 1, "harnesses": ["claude"], "artifacts": artifacts });

    for name in ALL_SKILLS.iter().chain(extra.iter()) {
        project.write(
            &format!("{CLAUDE_LINK}/{name}/SKILL.md"),
            &format!("---\nname: {name}\n---\n"),
        );
    }

    project.write(
        &format!("{STATE_DIRNAME}/{STATE_FILENAME}"),
        &format!("{}\n", crate::util::json::stringify_pretty(&state)),
    );
}

fn claude_project() -> Project {
    with_harnesses(&["claude"])
}

fn symlink(target: &str, at: &std::path::Path) {
    fs::symlink_dir(std::path::Path::new(target), at).expect("create a symlink");
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn replaces_a_directory_of_ambits_own_skills_with_the_link_without_adopt() {
    let project = claude_project();

    write_old_layout(&project, None);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    // ambit wrote everything that was in there, so turning the container into a link loses
    // nothing, which is why this one adoption is implicit.
    assert_eq!(readlink(&project, CLAUDE_LINK), format!("../{SKILLS_DIR}"));
    // And the skills survive the same run's pruning. Prior state owns `.claude/skills/<name>`,
    // which the new link now resolves through, so a prune that followed it would delete the
    // install it had just made.
    assert_eq!(skills_listed(&project), all_skills_sorted());
    assert!(
        project
            .state_artifacts()
            .iter()
            .any(|artifact| artifact.path == CLAUDE_LINK)
    );
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn refuses_when_one_hand_written_skill_sits_in_there_and_names_adopt() {
    let project = claude_project();

    write_old_layout(&project, Some("hand-written"));

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("refusing to overwrite unowned path"));
    assert!(result.stderr.contains(&format!(
        "{CLAUDE_LINK} exists but ambit did not create it, so it cannot be pointed at {SKILLS_DIR}"
    )));
    assert!(
        result
            .stderr
            .contains("run `ambit install --adopt` to take ownership")
    );
    // And the person's skill is still there, which is the whole point of refusing.
    assert!(
        project
            .read(&format!("{CLAUDE_LINK}/hand-written/SKILL.md"))
            .contains("name: hand-written")
    );
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn takes_it_over_under_adopt_which_is_what_the_refusal_offered() {
    let project = claude_project();

    write_old_layout(&project, Some("hand-written"));

    let result = project.cli(&["install", "--adopt"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(readlink(&project, CLAUDE_LINK), format!("../{SKILLS_DIR}"));
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn reports_a_dangling_symlink_as_unowned_rather_than_crashing() {
    // What dotagents leaves behind, and the shape that used to surface as "this is a bug in ambit":
    // `mkdir` on a path whose ancestor is a dangling link fails with ENOENT.
    let project = claude_project();

    fs::mkdir_p(&project.path(".claude")).expect("create .claude");
    symlink("../.agents/skills", &project.path(CLAUDE_LINK));

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("refusing to overwrite unowned path"));
    assert!(!result.stderr.contains("this is a bug in ambit"));
}

/// A dangling link an artifact has to be created *under* is different from one *at* a planned
/// path: `mkdir -p` cannot descend through it, so adopting the artifact would still fail.
#[test]
#[ignore = "needs B1, B2, B3"]
fn refuses_a_dangling_symlink_standing_where_a_parent_directory_belongs_and_says_what_to_move() {
    let project = claude_project();

    symlink("nowhere", &project.path(".agents"));

    for argv in [&["install"][..], &["install", "--adopt"][..]] {
        let result = project.cli(argv);

        assert_eq!(result.code, ExitCode::Config);
        assert!(
            result
                .stderr
                .contains("refusing to write under an unowned path")
        );
        assert!(result.stderr.contains(&format!(
            ".agents is not a directory ambit can write into, so {SKILLS_DIR}/{ENGINEERING_SKILL} cannot be created"
        )));
        assert!(
            result
                .stderr
                .contains("move .agents aside, or point it at a directory that exists")
        );
        // Never the internal-error wording, and `--adopt` is not offered, because it would not
        // help.
        assert!(!result.stderr.contains("this is a bug in ambit"));
        assert!(!result.stderr.contains("--adopt` to take ownership"));
    }
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn refuses_a_plain_file_standing_where_a_parent_directory_belongs() {
    let project = claude_project();

    project.write(".agents", "not a directory\n");

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains("refusing to write under an unowned path")
    );
    assert_eq!(project.read(".agents"), "not a directory\n");
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn writes_through_a_parent_directory_that_is_a_link_to_a_real_directory() {
    // A symlinked ancestor is only a problem when it dangles: one pointing at a directory is a
    // directory as far as writing into it goes.
    let project = claude_project();
    let elsewhere = project.root.join("elsewhere");

    fs::mkdir_p(&elsewhere).expect("create elsewhere");
    symlink(
        &relative(&project.dir, &elsewhere),
        &project.path(".agents"),
    );

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

    let mut names = fs::read_dir_names(&elsewhere.join("skills")).expect("skills elsewhere");

    names.sort();
    assert_eq!(names, all_skills_sorted());
}

#[test]
#[ignore = "needs B1, B2, B3"]
fn adopts_a_dangling_symlink_and_points_it_at_the_shared_directory() {
    let project = claude_project();

    fs::mkdir_p(&project.path(".claude")).expect("create .claude");
    symlink("../.agents/skills", &project.path(CLAUDE_LINK));

    let result = project.cli(&["install", "--adopt"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(readlink(&project, CLAUDE_LINK), format!("../{SKILLS_DIR}"));
    assert_eq!(skills_listed(&project), all_skills_sorted());
}
