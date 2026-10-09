use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use serde_json::json;

use super::fixture::*;
use super::*;
use crate::errors::ExitCode;
use crate::harness::definitions::CLAUDE;
use crate::model::documents::{DocumentFormat, DocumentShape};
use crate::model::state::{ArtifactKind, parse_state, serialize_state};
use crate::project::gitignore::{
    BLOCK_BEGIN, BLOCK_END, GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE,
};
use crate::project::lock::LOCK_FILENAME;
use crate::util::fs;
use crate::util::path::{join, relative, to_slash};

fn project() -> Project {
    let project = Project::new();

    project.write_profile(DEFAULT_PACKS, None, &[]);
    project
}

fn claude_adapter() -> ProfileAdapter {
    crate::harness::profile::adapter_for(&CLAUDE)
}

fn paths(root: &Path, mode: Option<ArtifactMode>) -> ProjectPaths {
    ProjectPaths {
        root: root.to_path_buf(),
        scope: None,
        mode,
    }
}

fn three_skill_tree() -> Vec<String> {
    strings(&[
        &format!("{ENGINEERING_SKILL}/SKILL.md"),
        &format!("{CORE_SKILL}/SKILL.md"),
        &format!("{FRONTEND_SKILL}/SKILL.md"),
    ])
}

fn three_skills() -> Vec<String> {
    strings(&[ENGINEERING_SKILL, CORE_SKILL, FRONTEND_SKILL])
}

fn skill(name: &str) -> String {
    format!("{SKILLS_DIR}/{name}")
}

fn home_env(home: &str) -> Env {
    Env::from([("HOME".to_owned(), home.to_owned())])
}

fn scope_of(project_dir: &str, env: &Env) -> (PathBuf, InstallScope) {
    let paths = project_paths(Path::new(project_dir), env, None);

    (paths.root, paths.scope.unwrap())
}

#[test]
fn installs_the_user_project_into_the_home_directory_and_anything_else_into_itself() {
    let env = home_env("/home/jane");

    assert_eq!(
        scope_of("/home/jane/.ambit", &env),
        (PathBuf::from("/home/jane"), InstallScope::User)
    );
    assert_eq!(
        scope_of("/home/jane/work/acme", &env),
        (PathBuf::from("/home/jane/work/acme"), InstallScope::Project)
    );
    assert_eq!(
        scope_of("/home/jane", &env),
        (PathBuf::from("/home/jane"), InstallScope::Project)
    );
    assert_eq!(
        scope_of("/home/jane/.ambit-backup", &env).1,
        InstallScope::Project
    );
}

#[test]
fn compares_resolved_paths_so_spelling_the_same_directory_differently_cannot_change_the_answer() {
    let env = home_env("/home/jane");

    assert_eq!(scope_of("/home/jane/.ambit/", &env).1, InstallScope::User);
    assert_eq!(
        scope_of("/home/jane/work/../.ambit", &env).1,
        InstallScope::User
    );
    assert_eq!(
        scope_of("/home/jane/.ambit", &home_env("/home/jane/")).1,
        InstallScope::User
    );
}

#[test]
fn falls_back_to_the_platforms_own_home_directory_when_home_is_unset() {
    let Some(home) = home_dir(&Env::new()) else {
        return;
    };
    let user = project_paths(&home.join(USER_PROJECT_DIRNAME), &Env::new(), None);

    assert_eq!(user.scope, Some(InstallScope::User));
    assert_eq!(user.root, home);
    assert_eq!(
        project_paths(&home.join("work"), &Env::new(), None).scope,
        Some(InstallScope::Project)
    );
}

#[test]
fn targets_one_directory_per_bundle_skill_and_one_config_file_and_touches_nothing() {
    let project = project();
    let plan = claude_adapter().plan(&project.bundle(), &paths(&project.dir, None));

    assert_eq!(
        plan.iter().map(PlannedArtifact::path).collect::<Vec<_>>(),
        [
            skill(ENGINEERING_SKILL).as_str(),
            &skill(CORE_SKILL),
            &skill(FRONTEND_SKILL),
            HOOK_DIR,
            CLAUDE_LINK,
            MCP_FILE,
            CLAUDE_SETTINGS,
        ]
    );

    let skills: Vec<&crate::harness::adapter::PlannedSkillDir> = plan
        .iter()
        .filter_map(|artifact| match artifact {
            PlannedArtifact::SkillDir(dir) => Some(dir),
            _ => None,
        })
        .collect();

    assert_eq!(
        skills.iter().map(|dir| dir.mode).collect::<Vec<_>>(),
        [ArtifactMode::Link; 3]
    );
    assert_eq!(
        skills[0].source,
        join(&project.catalog, "skills/code-review")
    );
    assert!(!project.exists(SKILLS_DIR));
    assert!(!project.exists(MCP_FILE));
}

#[test]
fn plans_the_mode_copy_and_link_ask_for_whatever_the_source_would_have_chosen() {
    let project = project();
    let bundle = project.bundle();
    let modes = |mode: ArtifactMode| {
        claude_adapter()
            .plan(&bundle, &paths(&project.dir, Some(mode)))
            .iter()
            .filter(|artifact| artifact.kind() == ArtifactKind::SkillDir)
            .filter_map(PlannedArtifact::mode)
            .collect::<Vec<_>>()
    };

    assert_eq!(modes(ArtifactMode::Copy), [ArtifactMode::Copy; 3]);
    assert_eq!(modes(ArtifactMode::Link), [ArtifactMode::Link; 3]);
}

#[test]
fn is_pure_planning_twice_yields_the_same_paths() {
    let project = project();
    let bundle = project.bundle();
    let at = paths(&project.dir, None);

    assert_eq!(
        claude_adapter().plan(&bundle, &at),
        claude_adapter().plan(&bundle, &at)
    );
}

#[test]
fn plans_no_server_config_file_for_a_bundle_with_no_servers() {
    let project = project();

    project.write_profile(&["core"], None, &[]);

    let plan = claude_adapter().plan(&project.bundle(), &paths(&project.dir, None));

    assert_eq!(
        plan.iter().map(PlannedArtifact::kind).collect::<Vec<_>>(),
        [
            ArtifactKind::SkillDir,
            ArtifactKind::SkillsLink,
            ArtifactKind::HarnessConfig
        ]
    );
    assert!(!plan.iter().any(|artifact| artifact.path() == MCP_FILE));
}

#[test]
fn writes_exactly_the_resolved_skill_directories() {
    let project = project();
    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(project.installed_skills(), three_skills());
    assert_eq!(project.tree(SKILLS_DIR), three_skill_tree());
}

#[test]
fn serves_the_catalogs_bytes_byte_for_byte_at_the_installed_path() {
    let project = project();

    project.cli(&["install"]);

    assert_eq!(
        project.read(&format!("{}/SKILL.md", skill(CORE_SKILL))),
        read_file(&join(&project.catalog, "skills/company-context/SKILL.md"))
    );
}

#[test]
fn installs_what_a_different_profile_resolves_to_and_nothing_more() {
    let project = project();

    project.write_profile(
        &[],
        None,
        &[&format!(
            "  - {{ skill: \"{CATALOG_NAME}/{FRONTEND_SKILL}\" }}"
        )],
    );
    project.cli(&["install"]);

    assert_eq!(project.installed_skills(), [FRONTEND_SKILL]);
}

#[test]
fn creates_no_skills_directory_for_an_empty_bundle() {
    let project = project();

    project.write_profile(&[], None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert!(!project.exists(SKILLS_DIR));
    assert_eq!(project.state_artifacts(), []);
}

fn default_state_artifacts() -> Vec<OwnedArtifact> {
    vec![
        owned(HOOK_DIR, ArtifactKind::HookDir, "link"),
        owned(&skill(ENGINEERING_SKILL), ArtifactKind::SkillDir, "link"),
        owned(&skill(CORE_SKILL), ArtifactKind::SkillDir, "link"),
        owned(&skill(FRONTEND_SKILL), ArtifactKind::SkillDir, "link"),
        config(
            CLAUDE_SETTINGS,
            DocumentFormat::Json,
            Some(DocumentShape::Array),
            hook_keys(CLAUDE_HOOK_ROOT),
        ),
        owned(CLAUDE_LINK, ArtifactKind::SkillsLink, "link"),
        config(
            MCP_FILE,
            DocumentFormat::Json,
            None,
            vec![format!("mcpServers.{PACKED_MCP}")],
        ),
    ]
}

#[test]
fn records_every_skill_directory_and_every_managed_config_key_as_owned() {
    let project = project();

    project.cli(&["install"]);

    assert_eq!(
        project.state(),
        State {
            version: 1,
            harnesses: strings(&["claude"]),
            artifacts: default_state_artifacts(),
        }
    );
}

#[test]
fn writes_a_byte_stable_state_file() {
    let project = project();

    project.cli(&["install"]);
    let first = project.read_state_file();

    project.cli(&["install"]);

    assert_eq!(project.read_state_file(), first);
}

#[test]
fn leaves_the_same_tree_behind_on_a_second_run() {
    let project = project();

    project.cli(&["install"]);
    let first = project.tree(SKILLS_DIR);

    let second = project.cli(&["install"]);

    assert_eq!(second.code, ExitCode::Success, "{}", second.stderr);
    assert_eq!(project.tree(SKILLS_DIR), first);
}

#[test]
fn replaces_an_owned_skill_directory_rather_than_merging_into_it() {
    let project = project();

    project.cli(&["install", "--copy"]);
    project.write(
        &format!("{}/stale.md", skill(CORE_SKILL)),
        "left over from an older catalog\n",
    );

    project.cli(&["install", "--copy"]);

    assert_eq!(project.tree(SKILLS_DIR), three_skill_tree());
}

#[test]
fn lists_what_it_wrote() {
    let project = project();
    let result = project.cli(&["install"]);

    let width = skill(CORE_SKILL).len();
    let row = |path: &str, rest: &str| format!("  {path:<width$}  {rest}");

    assert_eq!(
        result.stdout,
        [
            "harnesses (1)".to_owned(),
            "  claude".to_owned(),
            String::new(),
            "artifacts (7)".to_owned(),
            row(&skill(ENGINEERING_SKILL), "skill-dir       link"),
            row(&skill(CORE_SKILL), "skill-dir       link"),
            row(&skill(FRONTEND_SKILL), "skill-dir       link"),
            row(HOOK_DIR, "hook-dir        link"),
            row(CLAUDE_LINK, "skills-link     link"),
            row(MCP_FILE, "harness-config  -"),
            row(CLAUDE_SETTINGS, "harness-config  -"),
        ]
        .join("\n")
    );
}

#[test]
fn emits_machine_readable_output_carrying_no_absolute_paths() {
    let project = project();
    let result = project.cli(&["install", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).expect("JSON");

    assert_eq!(
        parsed,
        json!({
            "artifacts": [
                { "kind": "skill-dir", "mode": "link", "path": skill(ENGINEERING_SKILL) },
                { "kind": "skill-dir", "mode": "link", "path": skill(CORE_SKILL) },
                { "kind": "skill-dir", "mode": "link", "path": skill(FRONTEND_SKILL) },
                { "kind": "hook-dir", "mode": "link", "path": HOOK_DIR },
                { "kind": "skills-link", "mode": "link", "path": CLAUDE_LINK },
                {
                    "kind": "harness-config",
                    "managedKeys": [format!("mcpServers.{PACKED_MCP}")],
                    "path": MCP_FILE,
                },
                {
                    "kind": "harness-config",
                    "managedKeys": hook_keys(CLAUDE_HOOK_ROOT),
                    "path": CLAUDE_SETTINGS,
                },
            ],
            "harnesses": ["claude"],
            "skills": three_skills(),
            "skipped": [],
        })
    );
    assert!(!result.stdout.contains(&*project.root.to_string_lossy()));
}

#[test]
fn returns_the_bundle_it_installed() {
    let project = project();
    let result = install_project(&project.dir, &project.env, InstallOptions::default(), &[])
        .expect("an install");

    assert_eq!(
        result
            .bundle
            .skills
            .iter()
            .map(|skill| skill.name.clone())
            .collect::<Vec<_>>(),
        three_skills()
    );
    assert_eq!(result.harnesses, ["claude"]);
}

const CORE_SOURCE: &str = "skills/company-context";
const EDITED: &str = "---\nname: company-context\ntags: [core]\n---\n\n# edited\n";

fn read_source(project: &Project) -> String {
    read_file(&join(&project.catalog, &format!("{CORE_SOURCE}/SKILL.md")))
}

fn read_installed(project: &Project) -> String {
    project.read(&format!("{}/SKILL.md", skill(CORE_SKILL)))
}

fn recorded_mode(project: &Project, target: &str) -> Option<ArtifactMode> {
    project
        .state_artifacts()
        .into_iter()
        .find(|artifact| artifact.path == target)
        .and_then(|artifact| artifact.mode)
}

#[test]
fn symlinks_a_path_catalogs_skill_relatively_at_the_directory_the_catalog_holds() {
    let project = project();
    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

    let written = project.link_at(&skill(CORE_SKILL));
    let target = project.path(&skill(CORE_SKILL));

    assert_eq!(
        written.as_deref(),
        Some(
            to_slash(Path::new(&relative(
                target.parent().unwrap(),
                &join(&project.catalog, CORE_SOURCE)
            )))
            .as_str()
        )
    );
    assert!(written.unwrap().starts_with(".."));
    assert_eq!(
        recorded_mode(&project, &skill(CORE_SKILL)),
        Some(ArtifactMode::Link)
    );
}

#[test]
fn makes_editing_the_installed_skill_edit_the_tracked_source() {
    let project = project();

    project.cli(&["install"]);
    project.write(&format!("{}/SKILL.md", skill(CORE_SKILL)), EDITED);

    assert_eq!(read_source(&project), EDITED);
}

#[test]
fn copies_under_copy_so_editing_the_installed_skill_leaves_the_source_alone() {
    let project = project();
    let result = project.cli(&["install", "--copy"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(project.link_at(&skill(CORE_SKILL)), None);
    assert_eq!(
        recorded_mode(&project, &skill(CORE_SKILL)),
        Some(ArtifactMode::Copy)
    );

    let source = read_source(&project);

    project.write(&format!("{}/SKILL.md", skill(CORE_SKILL)), EDITED);

    assert_eq!(read_source(&project), source);
}

#[test]
fn replaces_a_copy_with_a_link_and_a_link_with_a_copy_when_the_mode_changes() {
    let project = project();

    assert_eq!(project.cli(&["install", "--copy"]).code, ExitCode::Success);

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);
    assert!(project.link_at(&skill(CORE_SKILL)).is_some());
    assert_eq!(
        recorded_mode(&project, &skill(CORE_SKILL)),
        Some(ArtifactMode::Link)
    );
    assert_eq!(read_installed(&project), read_source(&project));

    assert_eq!(project.cli(&["install", "--copy"]).code, ExitCode::Success);
    assert_eq!(project.link_at(&skill(CORE_SKILL)), None);
    assert_eq!(
        recorded_mode(&project, &skill(CORE_SKILL)),
        Some(ArtifactMode::Copy)
    );
    assert_eq!(project.tree(SKILLS_DIR), three_skill_tree());
}

#[test]
fn refuses_copy_and_link_together_rather_than_picking_one() {
    let project = project();

    for flags in [["--copy", "--link"], ["--link", "--copy"]] {
        let result = project.cli(&["install", flags[0], flags[1]]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(
            result.stderr.contains(&format!(
                "the argument '{}' cannot be used with '{}'",
                flags[0], flags[1]
            )),
            "{}",
            result.stderr
        );
        assert!(!project.exists(SKILLS_DIR));
    }
}

#[test]
fn unlinks_a_pruned_skill_without_following_the_link_into_the_catalog() {
    let project = project();

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);
    project.write_profile(&["core"], None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(project.installed_skills(), [CORE_SKILL]);
    assert!(!project.exists(&skill(ENGINEERING_SKILL)));
    assert!(exists(&join(
        &project.catalog,
        "skills/code-review/SKILL.md"
    )));
}

const BOTH_SERVERS: &[&str] = &[
    "function.engineering",
    "function.engineering.*",
    "project.acme",
];

fn tagged_server() -> serde_json::Value {
    json!({
        "type": "http",
        "url": "https://mcp.invalid/fixture",
        "headers": { "Authorization": format!("Bearer ${{{PACKED_KEY_VAR}}}") },
    })
}

fn fixture_server() -> serde_json::Value {
    json!({
        "command": "npx",
        "args": ["-y", "@acme/fixture-mcp"],
        "env": { "FIXTURE_API_KEY": "${FIXTURE_API_KEY}" },
    })
}

fn keys_of(value: &serde_json::Value) -> Vec<String> {
    value
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect()
}

#[test]
fn holds_exactly_the_tag_matched_server_and_the_requires_only_one() {
    let project = project();

    project.write_profile(BOTH_SERVERS, None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        project.mcp_config(),
        json!({ "mcpServers": { FIXTURE_MCP: fixture_server(), PACKED_MCP: tagged_server() } })
    );
}

#[test]
fn writes_no_file_at_all_when_the_bundle_selects_no_server() {
    let project = project();

    project.write_profile(&["core"], None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert!(!project.exists(MCP_FILE));
}

#[test]
fn writes_a_reference_rather_than_the_value_even_with_the_variable_set() {
    let mut project = project();

    project
        .env
        .insert(PACKED_KEY_VAR.to_owned(), "s3cret".to_owned());
    project.cli(&["install"]);

    assert_eq!(
        project.mcp_config(),
        json!({ "mcpServers": { PACKED_MCP: tagged_server() } })
    );
    assert!(!project.read(MCP_FILE).contains("s3cret"));
}

#[test]
fn writes_the_same_reference_whether_or_not_the_variable_is_set() {
    let mut project = project();

    project.cli(&["install"]);
    let unset = project.read(MCP_FILE);

    project
        .env
        .insert(PACKED_KEY_VAR.to_owned(), "s3cret".to_owned());
    project.cli(&["install"]);

    assert_eq!(project.read(MCP_FILE), unset);
    assert!(unset.contains(&format!("Bearer ${{{PACKED_KEY_VAR}}}")));
}

#[test]
fn gives_a_server_the_variable_names_it_reads_from_the_ones_the_machine_sets() {
    let project = project();

    project.write_catalog(
        "mcps/planner.yml",
        &[
            "name: planner",
            "",
            "transport:",
            "  stdio:",
            "    command: planner-mcp",
            "    env:",
            "      PLANNER_TOKEN: \"${ACME_PLANNER_TOKEN}\"",
            "      PLANNER_WORKSPACE: acme",
            "",
            "expects:",
            "  - env: ACME_PLANNER_TOKEN",
            "",
        ]
        .join("\n"),
    );
    project.write_profile(
        &[],
        None,
        &[&format!("  - {{ mcp: \"{CATALOG_NAME}/planner\" }}")],
    );

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        project.mcp_config(),
        json!({
            "mcpServers": {
                "planner": {
                    "command": "planner-mcp",
                    "env": { "PLANNER_TOKEN": "${ACME_PLANNER_TOKEN}", "PLANNER_WORKSPACE": "acme" },
                },
            },
        })
    );
}

#[test]
fn omits_args_and_headers_a_server_does_not_declare() {
    let project = project();

    project.write_catalog(
        "mcps/plain.yml",
        "name: plain\n\ntransport:\n  stdio:\n    command: plain-mcp\n",
    );
    project.write_catalog(
        "mcps/bare.yml",
        "name: bare\n\ntransport:\n  http:\n    url: https://bare.invalid/mcp\n",
    );
    project.write_profile(
        &[],
        None,
        &[
            &format!("  - {{ mcp: \"{CATALOG_NAME}/plain\" }}"),
            &format!("  - {{ mcp: \"{CATALOG_NAME}/bare\" }}"),
        ],
    );

    project.cli(&["install"]);

    assert_eq!(
        project.mcp_config(),
        json!({
            "mcpServers": {
                "bare": { "type": "http", "url": "https://bare.invalid/mcp" },
                "plain": { "command": "plain-mcp" },
            },
        })
    );
}

fn handmade_server() -> serde_json::Value {
    json!({ "command": "node", "args": ["./scripts/local-mcp.js"] })
}

fn pretty(value: &serde_json::Value) -> String {
    format!("{}\n", crate::util::json::stringify_pretty(value))
}

#[test]
fn leaves_a_hand_added_server_and_every_foreign_key_untouched() {
    let project = project();

    project.write(
        MCP_FILE,
        &pretty(
            &json!({ "mcpServers": { "handmade": handmade_server() }, "extra": { "kept": true } }),
        ),
    );
    project.write_profile(BOTH_SERVERS, None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

    let document = project.mcp_config();

    assert_eq!(
        document,
        json!({
            "mcpServers": {
                "handmade": handmade_server(),
                FIXTURE_MCP: fixture_server(),
                PACKED_MCP: tagged_server(),
            },
            "extra": { "kept": true },
        })
    );
    assert_eq!(keys_of(&document), ["mcpServers", "extra"]);
    assert_eq!(
        keys_of(&document["mcpServers"]),
        ["handmade", FIXTURE_MCP, PACKED_MCP]
    );
}

#[test]
fn records_only_the_keys_it_wrote_as_owned() {
    let project = project();

    project.write(
        MCP_FILE,
        &pretty(&json!({ "mcpServers": { "handmade": { "command": "node" } } })),
    );
    project.write_profile(BOTH_SERVERS, None, &[]);

    project.cli(&["install"]);

    assert_eq!(
        project
            .state_artifacts()
            .into_iter()
            .find(|artifact| artifact.path == MCP_FILE),
        Some(config(
            MCP_FILE,
            DocumentFormat::Json,
            None,
            vec![
                format!("mcpServers.{FIXTURE_MCP}"),
                format!("mcpServers.{PACKED_MCP}")
            ],
        ))
    );
}

#[test]
fn is_byte_identical_on_a_second_install() {
    let project = project();

    project.write_profile(BOTH_SERVERS, None, &[]);
    project.cli(&["install"]);
    let first = project.read(MCP_FILE);

    project.cli(&["install"]);

    assert_eq!(project.read(MCP_FILE), first);
}

#[test]
fn exits_2_rather_than_overwriting_a_file_it_cannot_parse() {
    let project = project();

    project.write(MCP_FILE, "{ not json\n");

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains(&format!("{MCP_FILE} is not valid JSON"))
    );
    assert_eq!(project.read(MCP_FILE), "{ not json\n");
}

#[test]
fn exits_2_when_the_servers_section_is_not_an_object() {
    let project = project();

    project.write(MCP_FILE, "{\"mcpServers\": []}\n");

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains(&format!(
        "\"mcpServers\" in {MCP_FILE} is not a JSON object"
    )));
    assert_eq!(project.read(MCP_FILE), "{\"mcpServers\": []}\n");
}

const HANDWRITTEN_IGNORE: &str = "node_modules/\n.env\n";

fn managed_block(project: &Project, file: &str) -> Vec<String> {
    let text = project.read(file);
    let lines: Vec<&str> = text.split('\n').collect();
    let start = lines
        .iter()
        .position(|line| line.starts_with(BLOCK_BEGIN))
        .unwrap_or_else(|| panic!("no managed block in {file}"));
    let end = lines
        .iter()
        .position(|line| line.starts_with(BLOCK_END))
        .expect("an end marker");

    assert!(end > start);
    strings(&lines[start + 1..end])
}

#[test]
fn lists_every_skill_directory_it_installed_in_the_shared_directorys_own_file() {
    let project = project();

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);

    assert_eq!(
        managed_block(&project, SHARED_GITIGNORE_FILE),
        [
            "/hooks/guard-secrets".to_owned(),
            format!("/skills/{ENGINEERING_SKILL}"),
            format!("/skills/{CORE_SKILL}"),
            format!("/skills/{FRONTEND_SKILL}"),
        ]
    );
}

#[test]
fn keeps_at_the_root_only_what_a_nested_file_cannot_reach() {
    let project = project();

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);

    assert_eq!(
        managed_block(&project, GITIGNORE_FILENAME),
        [".ambit/", CLAUDE_LINK]
    );
}

#[test]
fn leaves_the_nested_file_itself_tracked_so_a_clone_inherits_the_ignore_list() {
    let project = project();

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);

    for file in [GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE] {
        assert!(!managed_block(&project, file).contains(&SHARED_GITIGNORE_FILE.to_owned()));
    }
}

#[test]
fn ignores_a_linked_skill_too_which_git_would_otherwise_track_as_a_symlink() {
    let project = project();

    project.cli(&["install"]);

    assert!(project.link_at(&skill(CORE_SKILL)).is_some());
    assert!(
        managed_block(&project, SHARED_GITIGNORE_FILE).contains(&format!("/skills/{CORE_SKILL}"))
    );
}

#[test]
fn appends_to_a_gitignore_the_project_already_had_leaving_its_lines_untouched() {
    let project = project();

    project.write(GITIGNORE_FILENAME, HANDWRITTEN_IGNORE);

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);
    assert!(
        project
            .read(GITIGNORE_FILENAME)
            .starts_with(HANDWRITTEN_IGNORE)
    );
    assert!(managed_block(&project, GITIGNORE_FILENAME).contains(&".ambit/".to_owned()));
}

#[test]
fn drops_the_skill_a_narrowed_profile_no_longer_installs() {
    let project = project();

    project.cli(&["install"]);
    project.write_profile(&["core"], None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        managed_block(&project, SHARED_GITIGNORE_FILE),
        [format!("/skills/{CORE_SKILL}")]
    );
    assert_eq!(
        managed_block(&project, GITIGNORE_FILENAME),
        [".ambit/", CLAUDE_LINK]
    );
}

#[test]
fn rewrites_its_own_block_in_place_rather_than_adding_a_second_one() {
    let project = project();

    project.write(GITIGNORE_FILENAME, HANDWRITTEN_IGNORE);
    project.cli(&["install"]);
    project.write_profile(&["core"], None, &[]);

    project.cli(&["install"]);

    for file in [GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE] {
        let contents = project.read(file);

        assert_eq!(contents.matches(BLOCK_BEGIN).count(), 1, "{file}");
        assert!(!contents.contains(ENGINEERING_SKILL), "{file}");
    }
}

#[test]
fn writes_nothing_when_the_blocks_already_say_what_this_install_would_write() {
    let project = project();

    project.cli(&["install"]);
    let before: Vec<String> = [GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE]
        .iter()
        .map(|file| project.read(file))
        .collect();

    project.cli(&["install"]);

    for (index, file) in [GITIGNORE_FILENAME, SHARED_GITIGNORE_FILE]
        .iter()
        .enumerate()
    {
        assert_eq!(project.read(file), before[index], "{file}");
    }
}

#[test]
fn removes_the_nested_file_when_a_project_ends_up_installing_no_skills_at_all() {
    let project = project();

    project.cli(&["install"]);
    assert!(project.exists(SHARED_GITIGNORE_FILE));

    project.write_profile(&[], None, &[]);
    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);

    assert!(!project.exists(SHARED_GITIGNORE_FILE));
}

#[test]
fn exits_2_rather_than_guessing_at_an_unterminated_block_leaving_the_file_alone() {
    let project = project();
    let broken = format!("{HANDWRITTEN_IGNORE}{BLOCK_BEGIN}\n.ambit/\ncoverage/\n");

    project.write(GITIGNORE_FILENAME, &broken);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains(&format!(
        "{GITIGNORE_FILENAME} holds an unterminated ambit block"
    )));
    assert_eq!(project.read(GITIGNORE_FILENAME), broken);
    let mut sorted = three_skills();

    sorted.sort();
    assert_eq!(project.installed_skills(), sorted);
}

const OWN_SKILL: &str = "readwise-cli";
const OWN_PACK: &str = "own";

fn self_catalog_project() -> Project {
    let project = project();

    project.write(
        &format!("skills/{OWN_SKILL}/SKILL.md"),
        &[
            "---",
            &format!("name: {OWN_SKILL}"),
            "---",
            "",
            "# readwise",
            "",
        ]
        .join("\n"),
    );
    project.write(
        "mcps/custom.yml",
        &[
            "name: custom",
            "transport:",
            "  stdio:",
            "    command: custom-mcp",
            "",
        ]
        .join("\n"),
    );
    project.write(
        &format!("packs/{OWN_PACK}.yml"),
        &[
            &format!("name: {OWN_PACK}"),
            "description: What this project ships for itself.",
            "requires:",
            &format!("  - skill: {OWN_SKILL}"),
            "  - mcp: custom",
            "",
        ]
        .join("\n"),
    );
    project.write(
        "ambit.yml",
        &[
            "version: 1",
            "catalogs:",
            &format!("  - name: {CATALOG_NAME}"),
            "    source: path:../catalog",
            "  - name: local",
            "    source: path:.",
            "requires:",
            &requires_entry_from(OWN_PACK, "local"),
            "",
        ]
        .join("\n"),
    );
    project
}

#[test]
fn installs_the_projects_own_skill_and_server_and_records_both() {
    let project = self_catalog_project();
    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(project.installed_skills(), [OWN_SKILL]);
    assert_eq!(
        project.mcp_config(),
        json!({ "mcpServers": { "custom": { "command": "custom-mcp" } } })
    );
    assert_eq!(
        project.state_artifacts(),
        [
            owned(&skill(OWN_SKILL), ArtifactKind::SkillDir, "link"),
            owned(CLAUDE_LINK, ArtifactKind::SkillsLink, "link"),
            config(
                MCP_FILE,
                DocumentFormat::Json,
                None,
                strings(&["mcpServers.custom"])
            ),
        ]
    );
}

#[test]
fn links_the_skill_to_the_projects_own_directory_not_to_a_copy_of_it() {
    let project = self_catalog_project();

    project.cli(&["install"]);

    assert_eq!(
        project.link_at(&skill(OWN_SKILL)),
        Some(to_slash(Path::new(&relative(
            &project.path(SKILLS_DIR),
            &project.path(&format!("skills/{OWN_SKILL}"))
        ))))
    );
}

#[test]
fn names_the_catalog_local_since_that_is_what_the_config_called_it() {
    let project = self_catalog_project();
    let bundle = project.bundle();

    assert_eq!(
        bundle
            .skills
            .iter()
            .map(|skill| skill.catalog.as_str())
            .collect::<Vec<_>>(),
        ["local"]
    );
    assert_eq!(
        bundle
            .mcps
            .iter()
            .map(|mcp| mcp.catalog.as_str())
            .collect::<Vec<_>>(),
        ["local"]
    );
}

#[test]
fn exits_2_for_an_mcp_entity_whose_transport_names_no_kind_or_two_kinds() {
    for transport in [
        "transport: {}",
        "transport:\n  stdio:\n    command: npx\n  http:\n    url: https://x.invalid",
    ] {
        let project = project();

        project.write_catalog("mcps/broken.yml", &format!("name: broken\n{transport}\n"));

        let result = project.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(result.stderr.contains("supported kinds: http, stdio"));
        assert!(!project.exists(MCP_FILE));
        assert!(!project.exists(SKILLS_DIR));
    }
}

#[test]
fn exits_2_for_a_harness_with_no_adapter() {
    let project = project();

    project.write_profile(&["core"], Some(&["zed"]), &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("unknown harness \"zed\""));
    assert!(
        result
            .stderr
            .contains("claude, codex, copilot, cursor, devin, gemini, grok, kiro, opencode")
    );
    assert!(!project.exists(SKILLS_DIR));
}

#[test]
fn names_every_shipped_adapter_when_one_is_unknown() {
    let error = adapters_for(&strings(&["claude", "zed"]))
        .err()
        .expect("a refusal");

    assert_eq!(error.code, ExitCode::Config);
    assert_eq!(
        error.format(),
        [
            "error: unknown harness \"zed\" (ambit.yml)",
            "       this build ships adapters for: claude, codex, copilot, cursor, devin, gemini, grok, kiro, opencode",
            "       remove it from `harnesses`, or correct the spelling",
        ]
        .join("\n")
    );
}

#[test]
fn exits_2_when_the_project_has_no_config() {
    let project = project();

    fs::rm_rf(&project.path("ambit.yml")).expect("remove the config");

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("no ambit config"));
}

#[test]
fn exits_2_rather_than_trusting_an_unreadable_state_file() {
    let project = project();

    project.cli(&["install"]);
    project.write(
        ".ambit/state.json",
        "{\"version\": 1, \"harnesses\": [\"claude\"], \"artifacts\": [{\"kind\": \"nonsense\"}]}\n",
    );

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("not a valid ambit state file"));
}

fn files_section(lock: &str, root: &str, shared: &str) -> String {
    [
        "files (3)".to_owned(),
        format!("  {LOCK_FILENAME}          {lock}"),
        format!("  {GITIGNORE_FILENAME}          {root}"),
        format!("  {SHARED_GITIGNORE_FILE}  {shared}"),
    ]
    .join("\n")
}

fn extra_sections(lock: &str, root: &str, shared: &str) -> String {
    [
        "pruned (0)".to_owned(),
        "  (none)".to_owned(),
        String::new(),
        files_section(lock, root, shared),
    ]
    .join("\n")
}

#[test]
fn writes_nothing_at_all() {
    let project = project();
    let result = project.cli(&["install", "--dry-run"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        project.snapshot().into_keys().collect::<Vec<_>>(),
        ["ambit.yml"]
    );
    assert!(!project.exists(SKILLS_DIR));
}

#[test]
fn prints_the_rows_the_install_goes_on_to_print_plus_what_only_a_preview_can_say() {
    let project = project();
    let preview = project.cli(&["install", "--dry-run"]);
    let installed = project.cli(&["install"]);

    assert_eq!(installed.code, ExitCode::Success, "{}", installed.stderr);
    assert_eq!(
        preview.stdout,
        format!(
            "{}\n\n{}",
            installed.stdout,
            extra_sections("changed", "changed", "changed")
        )
    );
}

#[test]
fn reports_every_derived_file_as_unchanged_once_the_project_is_installed() {
    let project = project();

    project.cli(&["install"]);

    let result = project.cli(&["install", "--dry-run"]);

    assert!(
        result
            .stdout
            .contains(&extra_sections("unchanged", "unchanged", "unchanged"))
    );
}

#[test]
fn reports_the_root_block_unchanged_and_the_nested_one_stale_when_only_the_bundle_narrowed() {
    let project = project();

    project.cli(&["install"]);
    project.write_profile(&["core"], None, &[]);

    let result = project.cli(&["install", "--dry-run"]);

    assert!(
        result
            .stdout
            .contains(&files_section("changed", "unchanged", "changed"))
    );
}

#[test]
fn reports_what_the_install_would_remove_and_removes_none_of_it() {
    let project = project();

    project.cli(&["install"]);
    project.write_profile(&["core"], None, &[]);
    let before = project.snapshot();

    let result = project.cli(&["install", "--dry-run", "--json"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).expect("JSON");

    assert_eq!(
        parsed,
        json!({
            "artifacts": [
                { "kind": "skill-dir", "mode": "link", "path": skill(CORE_SKILL) },
                { "kind": "skills-link", "mode": "link", "path": CLAUDE_LINK },
                { "kind": "harness-config", "managedKeys": [core_hook_key()], "path": CLAUDE_SETTINGS },
            ],
            "gitignore": [
                { "changed": false, "file": GITIGNORE_FILENAME },
                { "changed": true, "file": SHARED_GITIGNORE_FILE },
            ],
            "harnesses": ["claude"],
            "lockChanged": true,
            "pruned": [
                { "kind": "hook-dir", "path": HOOK_DIR },
                { "kind": "skill-dir", "path": skill(ENGINEERING_SKILL) },
                { "kind": "skill-dir", "path": skill(FRONTEND_SKILL) },
                {
                    "kind": "harness-config",
                    "managedKeys": [engineering_hook_key(CLAUDE_HOOK_ROOT)],
                    "path": CLAUDE_SETTINGS,
                },
                {
                    "kind": "harness-config",
                    "managedKeys": [format!("mcpServers.{PACKED_MCP}")],
                    "path": MCP_FILE,
                },
            ],
            "skills": [CORE_SKILL],
            "skipped": [],
        })
    );
    assert_eq!(project.snapshot(), before);
    assert_eq!(project.installed_skills(), three_skills());
}

#[test]
fn refuses_an_unowned_target_rather_than_previewing_an_install_that_would_stop() {
    let project = project();

    project.write(
        &format!("{}/SKILL.md", skill(CORE_SKILL)),
        "---\nname: hand-written\n---\n",
    );

    let result = project.cli(&["install", "--dry-run"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("refusing to overwrite unowned path"));
}

#[test]
fn still_refuses_a_stale_lock_under_frozen_since_refusing_writes_nothing() {
    let project = project();

    project.cli(&["install"]);
    project.write_profile(&["core"], None, &[]);

    let result = project.cli(&["install", "--dry-run", "--frozen"]);

    assert_eq!(result.code, ExitCode::Drift);
    assert!(
        result
            .stderr
            .contains(&format!("{LOCK_FILENAME} is out of date"))
    );
}

const HANDWRITTEN_SKILL: &str = "---\nname: hand-written\n---\n\n# not ambit's\n";
const STRAY: &str = "notes nobody told ambit about\n";
const STATE_FILE: &str = ".ambit/state.json";

fn write_unowned_skill_dir(project: &Project) {
    project.write(
        &format!("{}/SKILL.md", skill(CORE_SKILL)),
        HANDWRITTEN_SKILL,
    );
    project.write(&format!("{}/notes.md", skill(CORE_SKILL)), STRAY);
}

fn write_unowned_server(project: &Project) -> String {
    let contents = pretty(&json!({
        "mcpServers": {
            PACKED_MCP: { "command": "node", "args": ["./tagged.js"] },
            "kept": { "command": "keep" },
        },
    }));

    project.write(MCP_FILE, &contents);
    contents
}

#[test]
fn refuses_to_overwrite_a_skill_directory_it_does_not_own() {
    let project = project();

    write_unowned_skill_dir(&project);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("refusing to overwrite unowned path"));
    assert!(result.stderr.contains(&format!(
        "{} exists but ambit did not create it",
        skill(CORE_SKILL)
    )));
    assert!(
        result
            .stderr
            .contains("run `ambit install --adopt` to take ownership")
    );
}

#[test]
fn leaves_an_unowned_directory_byte_identical_and_installs_nothing_else_either() {
    let project = project();

    write_unowned_skill_dir(&project);
    project.cli(&["install"]);

    assert_eq!(
        project.tree(SKILLS_DIR),
        [
            format!("{CORE_SKILL}/SKILL.md"),
            format!("{CORE_SKILL}/notes.md")
        ]
    );
    assert_eq!(
        project.read(&format!("{}/SKILL.md", skill(CORE_SKILL))),
        HANDWRITTEN_SKILL
    );
    assert!(!project.exists(MCP_FILE));
    assert!(!project.exists(LOCK_FILENAME));
    assert!(!project.exists(STATE_FILE));
    assert!(!project.exists(GITIGNORE_FILENAME));
}

#[test]
fn refuses_a_plain_file_sitting_where_a_skill_directory_belongs() {
    let project = project();

    project.write(&skill(CORE_SKILL), STRAY);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("refusing to overwrite unowned path"));
    assert_eq!(project.read(&skill(CORE_SKILL)), STRAY);
}

#[test]
fn does_not_read_owning_one_skill_as_permission_to_overwrite_another() {
    let project = project();

    project.write_profile(&["core"], None, &[]);
    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);
    project.write_profile(DEFAULT_PACKS, None, &[]);
    project.write(
        &format!("{}/SKILL.md", skill(ENGINEERING_SKILL)),
        HANDWRITTEN_SKILL,
    );

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains(&format!("{} exists", skill(ENGINEERING_SKILL)))
    );
    assert_eq!(
        project.read(&format!("{}/SKILL.md", skill(ENGINEERING_SKILL))),
        HANDWRITTEN_SKILL
    );
}

#[test]
fn refuses_to_overwrite_a_server_key_it_does_not_own() {
    let project = project();
    let contents = write_unowned_server(&project);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("refusing to overwrite unowned key"));
    assert!(
        result
            .stderr
            .contains(&format!("\"mcpServers.{PACKED_MCP}\" in {MCP_FILE}"))
    );
    assert!(
        result
            .stderr
            .contains(&format!("remove it from {MCP_FILE}"))
    );
    assert_eq!(project.read(MCP_FILE), contents);
    assert!(!project.exists(SKILLS_DIR));
}

#[test]
fn replaces_an_adopted_skill_directory_rather_than_copying_into_it() {
    let project = project();

    write_unowned_skill_dir(&project);

    let result = project.cli(&["install", "--adopt"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(project.tree(SKILLS_DIR), three_skill_tree());
    assert_eq!(
        project.read(&format!("{}/SKILL.md", skill(CORE_SKILL))),
        read_file(&join(&project.catalog, "skills/company-context/SKILL.md"))
    );
    assert!(
        project
            .state_artifacts()
            .iter()
            .any(|artifact| artifact.path == skill(CORE_SKILL))
    );
}

#[test]
fn adopts_a_colliding_server_key_while_leaving_foreign_keys_alone() {
    let project = project();

    write_unowned_server(&project);

    let result = project.cli(&["install", "--adopt"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        project.mcp_config(),
        json!({ "mcpServers": { PACKED_MCP: tagged_server(), "kept": { "command": "keep" } } })
    );
    assert_eq!(
        project
            .state_artifacts()
            .into_iter()
            .find(|artifact| artifact.path == MCP_FILE)
            .and_then(|artifact| artifact.managed_keys),
        Some(vec![format!("mcpServers.{PACKED_MCP}")])
    );
}

#[test]
fn needs_adopt_only_once_the_second_install_owns_what_the_first_adopted() {
    let project = project();

    write_unowned_skill_dir(&project);
    write_unowned_server(&project);
    assert_eq!(project.cli(&["install", "--adopt"]).code, ExitCode::Success);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
}

#[test]
fn changes_nothing_when_there_is_nothing_to_adopt() {
    let project = project();

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);
    let state = project.read_state_file();
    let servers = project.read(MCP_FILE);

    let result = project.cli(&["install", "--adopt"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(project.read_state_file(), state);
    assert_eq!(project.read(MCP_FILE), servers);
}

const HANDMADE_SKILL: &str = "hand-written";

fn write_foreign_skill_dir(project: &Project) {
    project.write(
        &format!("{}/SKILL.md", skill(HANDMADE_SKILL)),
        &format!("---\nname: {HANDMADE_SKILL}\n---\n"),
    );
}

fn install(project: &Project) -> InstallResult {
    install_project(&project.dir, &project.env, InstallOptions::default(), &[]).expect("an install")
}

fn pruned(path: &str, kind: ArtifactKind, keys: Option<Vec<String>>) -> PrunedArtifact {
    PrunedArtifact {
        path: path.to_owned(),
        kind,
        managed_keys: keys,
        format: None,
        shape: None,
    }
}

#[test]
fn removes_the_skill_directories_the_new_bundle_no_longer_selects() {
    let project = project();

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);
    project.write_profile(&["core"], None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(project.installed_skills(), [CORE_SKILL]);
    assert_eq!(project.tree(SKILLS_DIR), [format!("{CORE_SKILL}/SKILL.md")]);
}

#[test]
fn stops_claiming_what_it_removed() {
    let project = project();

    project.cli(&["install"]);
    project.write_profile(&["core"], None, &[]);
    project.cli(&["install"]);

    assert_eq!(
        project.state_artifacts(),
        [
            owned(&skill(CORE_SKILL), ArtifactKind::SkillDir, "link"),
            config(
                CLAUDE_SETTINGS,
                DocumentFormat::Json,
                Some(DocumentShape::Array),
                vec![core_hook_key()],
            ),
            owned(CLAUDE_LINK, ArtifactKind::SkillsLink, "link"),
        ]
    );
}

#[test]
fn reports_what_it_removed_by_path() {
    let project = project();

    install(&project);
    project.write_profile(&["core"], None, &[]);

    let result = install(&project);

    assert_eq!(
        result.pruned,
        [
            pruned(HOOK_DIR, ArtifactKind::HookDir, None),
            pruned(&skill(ENGINEERING_SKILL), ArtifactKind::SkillDir, None),
            pruned(&skill(FRONTEND_SKILL), ArtifactKind::SkillDir, None),
            pruned(
                CLAUDE_SETTINGS,
                ArtifactKind::HarnessConfig,
                Some(vec![engineering_hook_key(CLAUDE_HOOK_ROOT)])
            ),
            pruned(
                MCP_FILE,
                ArtifactKind::HarnessConfig,
                Some(vec![format!("mcpServers.{PACKED_MCP}")])
            ),
        ]
    );
}

#[test]
fn removes_only_the_server_keys_the_new_bundle_dropped() {
    let project = project();

    project.write_profile(BOTH_SERVERS, None, &[]);
    project.cli(&["install"]);
    project.write_profile(
        &["function.engineering", "function.engineering.*"],
        None,
        &[],
    );

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(keys_of(&project.mcp_config()["mcpServers"]), [PACKED_MCP]);
    assert_eq!(project.state_artifacts(), default_state_artifacts());
}

#[test]
fn empties_the_servers_section_rather_than_deleting_a_file_it_co_owns() {
    let project = project();

    project.cli(&["install"]);
    project.write_profile(&["core"], None, &[]);
    project.cli(&["install"]);

    assert_eq!(project.mcp_config(), json!({ "mcpServers": {} }));
    assert!(
        !project
            .state_artifacts()
            .iter()
            .any(|artifact| artifact.path == MCP_FILE)
    );
}

#[test]
fn prunes_around_a_hand_added_server_and_every_foreign_key() {
    let project = project();

    project.write(
        MCP_FILE,
        &pretty(
            &json!({ "mcpServers": { "handmade": handmade_server() }, "extra": { "kept": true } }),
        ),
    );
    project.cli(&["install"]);
    project.write_profile(&["core"], None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        project.mcp_config(),
        json!({ "mcpServers": { "handmade": handmade_server() }, "extra": { "kept": true } })
    );
}

#[test]
fn leaves_a_skill_directory_no_state_claims_alone() {
    let project = project();

    project.cli(&["install"]);
    write_foreign_skill_dir(&project);
    project.write_profile(&["core"], None, &[]);
    project.cli(&["install"]);

    assert_eq!(project.installed_skills(), [CORE_SKILL, HANDMADE_SKILL]);
    assert!(
        project
            .tree(SKILLS_DIR)
            .contains(&format!("{HANDMADE_SKILL}/SKILL.md"))
    );
}

#[test]
fn removes_a_skill_an_entry_named_once_the_entry_goes() {
    let project = project();

    project.write_profile(
        &[],
        None,
        &[&format!(
            "  - {{ skill: \"{CATALOG_NAME}/{PROJECT_SKILL}\" }}"
        )],
    );
    project.cli(&["install"]);
    assert_eq!(project.installed_skills(), [PROJECT_SKILL, CORE_SKILL]);
    project.write_profile(&[], None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(project.installed_skills(), Vec::<String>::new());
    assert_eq!(project.mcp_config(), json!({ "mcpServers": {} }));
    assert_eq!(project.state_artifacts(), []);
}

#[test]
fn changes_nothing_when_the_bundle_is_unchanged() {
    let project = project();

    project.write_profile(BOTH_SERVERS, None, &[]);
    project.cli(&["install"]);
    let servers = project.read(MCP_FILE);
    let state = project.read_state_file();

    let result = install(&project);

    assert_eq!(result.pruned, []);
    assert_eq!(project.read(MCP_FILE), servers);
    assert_eq!(project.read_state_file(), state);
}

#[test]
fn succeeds_when_what_it_owned_is_already_gone() {
    let project = project();

    project.cli(&["install"]);
    fs::rm_rf(&project.path(&skill(ENGINEERING_SKILL))).expect("remove a skill");
    fs::rm_rf(&project.path(MCP_FILE)).expect("remove the server file");
    project.write_profile(&["core"], None, &[]);

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(project.installed_skills(), [CORE_SKILL]);
    assert!(!project.exists(MCP_FILE));
}

#[test]
fn exits_2_rather_than_guessing_at_a_managed_key_that_names_no_section() {
    let project = project();

    project.cli(&["install"]);
    let mut state = project.state();

    for artifact in &mut state.artifacts {
        if artifact.kind == ArtifactKind::HarnessConfig && artifact.path == MCP_FILE {
            artifact.managed_keys = Some(vec![
                format!("mcpServers.{PACKED_MCP}"),
                PACKED_MCP.to_owned(),
            ]);
        }
    }

    project.write(STATE_FILE, &serialize_state(&state));

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains(&format!("cannot prune \"{PACKED_MCP}\" from {MCP_FILE}"))
    );
}

fn project_files() -> Vec<String> {
    let mut files = vec![
        STATE_FILE.to_owned(),
        format!("{}/SKILL.md", skill(ENGINEERING_SKILL)),
        format!("{}/SKILL.md", skill(CORE_SKILL)),
        format!("{}/SKILL.md", skill(FRONTEND_SKILL)),
        format!("{HOOK_DIR}/hook.yml"),
        format!("{HOOK_DIR}/guard.sh"),
        CLAUDE_LINK.to_owned(),
        MCP_FILE.to_owned(),
        CLAUDE_SETTINGS.to_owned(),
        LOCK_FILENAME.to_owned(),
        GITIGNORE_FILENAME.to_owned(),
        SHARED_GITIGNORE_FILE.to_owned(),
        "ambit.yml".to_owned(),
    ];

    files.sort();
    files
}

#[test]
fn changes_no_bytes_on_a_second_identical_install() {
    let project = project();

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);
    let before = project.snapshot();

    assert_eq!(before.keys().cloned().collect::<Vec<_>>(), project_files());

    let second = project.cli(&["install"]);

    assert_eq!(second.code, ExitCode::Success, "{}", second.stderr);
    assert_eq!(project.snapshot(), before);
}

#[test]
fn changes_no_bytes_on_a_second_install_of_a_project_holding_content_it_does_not_own() {
    let project = project();

    project.write(
        MCP_FILE,
        &pretty(
            &json!({ "mcpServers": { "handmade": { "command": "node" } }, "extra": { "kept": true } }),
        ),
    );
    project.write(
        &format!("{}/SKILL.md", skill("hand-written")),
        "---\nname: hand-written\n---\n",
    );

    assert_eq!(project.cli(&["install"]).code, ExitCode::Success);
    let before = project.snapshot();

    let second = project.cli(&["install"]);

    assert_eq!(second.code, ExitCode::Success, "{}", second.stderr);
    assert_eq!(project.snapshot(), before);
}

#[test]
fn prints_the_same_report_twice() {
    let project = project();
    let first = project.cli(&["install"]);

    assert_eq!(project.cli(&["install"]).stdout, first.stdout);
}

#[test]
fn treats_an_absent_file_as_owning_nothing() {
    let project = project();

    assert_eq!(
        crate::model::state::read_state(&project.dir).expect("a state"),
        State::empty()
    );
}

#[test]
fn rejects_a_state_file_from_a_future_version() {
    let error = parse_state(
        "{\"version\": 2, \"harnesses\": [], \"artifacts\": []}",
        "state.json",
    )
    .expect_err("a refusal");

    assert!(
        error.format().contains("unsupported state version 2"),
        "{}",
        error.format()
    );
}

#[test]
fn plans_each_shared_artifact_once_across_adapters() {
    let adapters = adapters_for(&strings(&["claude", "cursor"])).expect("adapters");
    let bundle = Bundle {
        skills: vec![crate::model::catalog::MergedSkill {
            name: CORE_SKILL.to_owned(),
            path: CORE_SOURCE.to_owned(),
            description: None,
            requires: Vec::new(),
            expects: Vec::new(),
            catalog: CATALOG_NAME.to_owned(),
            commit: None,
            catalog_root: PathBuf::from("/catalog"),
        }],
        ..Bundle::default()
    };
    let plans = plan_for(&adapters, &bundle, &paths(Path::new("/project"), None));

    assert_eq!(
        plans
            .iter()
            .map(|plan| (
                plan.adapter.name().to_owned(),
                plan.plan
                    .iter()
                    .map(|artifact| artifact.path().to_owned())
                    .collect::<Vec<_>>()
            ))
            .collect::<Vec<_>>(),
        [
            (
                "claude".to_owned(),
                vec![skill(CORE_SKILL), CLAUDE_LINK.to_owned()]
            ),
            ("cursor".to_owned(), Vec::new()),
        ]
    );
}
