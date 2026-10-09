use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;
use crate::errors::ExitCode;
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::{run_cli, tempdir, test_env};
use crate::util::fs::{read_dir_names, read_text};

const CATALOG_NAME: &str = "company";
const SKILLS_DIR: &str = ".agents/skills";
const CLAUDE_LINK: &str = ".claude/skills";
const MCP_FILE: &str = ".mcp.json";
const LOCK_FILE: &str = "ambit.lock";
const STATE_FILE: &str = ".ambit/state.json";
const GITIGNORE_FILE: &str = ".gitignore";

const CORE_SKILL: &str = "company-context";
const ENGINEERING_SKILL: &str = "code-review";
const FRONTEND_SKILL: &str = "design-tokens";

const HOOK_TARGET: &str = ".agents/hooks/guard-secrets";
const CLAUDE_SETTINGS: &str = ".claude/settings.json";

const FIGMA_VAR: &str = "ACME_FIGMA_TOKEN";
const TAGGED_VAR: &str = "LINTER_API_KEY";

const HOOK: &str = "notify";
const HOOK_VAR: &str = "NOTIFY_WEBHOOK";

const HOOK_PACK: &str = "harness.cases";

const HOOK_LINES: &[&str] = &["event: Stop", "type: command", "command: ./bin/notify"];

fn core_target() -> String {
    format!("{SKILLS_DIR}/{CORE_SKILL}")
}

fn frontend_target() -> String {
    format!("{SKILLS_DIR}/{FRONTEND_SKILL}")
}

fn engineering_target() -> String {
    format!("{SKILLS_DIR}/{ENGINEERING_SKILL}")
}

const HEALTHY_REPORT: &str = "checks (6)
  expects    ok
  lock       ok
  ownership  ok
  drift      ok
  mode       ok
  harness    ok

failures (0)
  (none)

warnings (0)
  (none)";

fn requires_entry(pack: &str) -> String {
    format!("  - {{ pack: \"{CATALOG_NAME}/{pack}\" }}")
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("create the parent");
    fs::write(path, text).expect("write the file");
}

fn strip(text: &str) -> String {
    text.strip_suffix('\n').unwrap_or(text).to_owned()
}

struct Out {
    code: ExitCode,
    stdout: String,
    stderr: String,
}

struct Fixture {
    _root: tempfile::TempDir,
    root: PathBuf,
    catalog_dir: PathBuf,
    project_dir: PathBuf,
    env: RefCell<Env>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempdir();
        let root = dir.path().to_path_buf();
        let fixture = Self {
            catalog_dir: root.join("catalog"),
            project_dir: root.join("project"),
            env: RefCell::new(test_env(&root)),
            root,
            _root: dir,
        };

        build_fixture_catalog(&fixture.catalog_dir).expect("build the fixture catalog");
        fs::create_dir_all(&fixture.project_dir).expect("create the project");
        fixture.write_profile(&["core", "function.engineering", "function.engineering.*"]);
        fixture
    }

    fn stub_env(&self, name: &str, value: Option<&str>) {
        let mut env = self.env.borrow_mut();

        match value {
            Some(value) => env.insert(name.to_owned(), value.to_owned()),
            None => env.remove(name),
        };
    }

    fn write_profile(&self, packs: &[&str]) {
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

        write(
            &self.project_dir.join("ambit.yml"),
            &format!(
                "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nrequires: {list}\n"
            ),
        );
    }

    fn write_hook_profile(&self, harnesses: &[&str], hooks: &[&str]) {
        if !hooks.is_empty() {
            let mut text = vec![format!("name: {HOOK}")];

            text.extend(hooks.iter().map(|&line| line.to_owned()));
            text.push(String::new());
            write(
                &self.catalog_dir.join("hooks").join(HOOK).join("hook.yml"),
                &text.join("\n"),
            );
            write(
                &self
                    .catalog_dir
                    .join("packs")
                    .join(format!("{HOOK_PACK}.yml")),
                &[
                    format!("name: {HOOK_PACK}"),
                    "description: The hook these cases install.".to_owned(),
                    "requires:".to_owned(),
                    format!("  - hook: {HOOK}"),
                    String::new(),
                ]
                .join("\n"),
            );
        }

        let requires = if hooks.is_empty() {
            "[]".to_owned()
        } else {
            format!("\n{}", requires_entry(HOOK_PACK))
        };

        write(
            &self.project_dir.join("ambit.yml"),
            &format!(
                "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nharnesses: [{}]\nrequires: {requires}\n",
                harnesses.join(", ")
            ),
        );
    }

    fn cli(&self, args: &[&str]) -> Out {
        let project = self.project_dir.to_string_lossy().into_owned();
        let mut argv: Vec<&str> = args.to_vec();

        argv.extend(["--project", &project]);

        let result = run_cli(&argv, &self.root, &self.env.borrow());

        Out {
            code: result.code,
            stdout: strip(&result.stdout),
            stderr: strip(&result.stderr),
        }
    }

    fn install(&self, args: &[&str]) {
        let mut argv = vec!["install"];

        argv.extend(args);

        let result = self.cli(&argv);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    }

    fn diagnose(&self) -> DoctorReport {
        diagnose_project(
            &self.project_dir,
            &self.env.borrow(),
            DoctorOptions::default(),
        )
        .expect("diagnoses")
    }

    fn snapshot(&self) -> BTreeMap<String, String> {
        fn walk(current: &Path, relative: &str, found: &mut BTreeMap<String, String>) {
            for entry in read_dir_names(current).expect("list the directory") {
                let within = if relative.is_empty() {
                    entry.clone()
                } else {
                    format!("{relative}/{entry}")
                };
                let absolute = current.join(&entry);

                if fs::metadata(&absolute).expect("stat").is_dir() {
                    walk(&absolute, &within, found);
                } else {
                    found.insert(within, read_text(&absolute).expect("read the file"));
                }
            }
        }

        let mut found = BTreeMap::new();

        walk(&self.project_dir, "", &mut found);
        found
    }

    fn findings(&self) -> Vec<String> {
        self.diagnose()
            .findings
            .iter()
            .map(|finding| {
                format!(
                    "{}/{}: {}",
                    finding.check, finding.severity, finding.message
                )
            })
            .collect()
    }

    fn detail_of(&self, needle: &str) -> Vec<String> {
        let found: Vec<DoctorFinding> = self
            .diagnose()
            .findings
            .into_iter()
            .filter(|finding| finding.message.contains(needle))
            .collect();

        assert_eq!(found.len(), 1, "findings matching {needle}: {found:?}");
        found[0].detail.clone()
    }

    fn checks(&self) -> Vec<String> {
        self.diagnose()
            .checks
            .iter()
            .map(|result| format!("{}={}", result.check, result.status))
            .collect()
    }
}

fn s(text: &str) -> String {
    text.to_owned()
}

fn installed() -> Fixture {
    let fixture = Fixture::new();

    fixture.stub_env(FIGMA_VAR, Some("figma-token"));
    fixture.stub_env(TAGGED_VAR, Some("tagged-key"));
    fixture.install(&[]);
    fixture
}

fn finding(check: DoctorCheck, severity: DoctorSeverity) -> DoctorFinding {
    DoctorFinding {
        check,
        severity,
        message: s("m"),
        detail: Vec::new(),
    }
}

#[test]
fn derives_each_checks_verdict_from_its_worst_finding() {
    let results = check_results(&[
        finding(DoctorCheck::Lock, DoctorSeverity::Fail),
        finding(DoctorCheck::Mode, DoctorSeverity::Warn),
        finding(DoctorCheck::Drift, DoctorSeverity::Warn),
        finding(DoctorCheck::Drift, DoctorSeverity::Fail),
    ]);

    assert_eq!(
        results
            .iter()
            .map(|result| format!("{}={}", result.check, result.status))
            .collect::<Vec<_>>(),
        [
            "expects=ok",
            "lock=fail",
            "ownership=ok",
            "drift=fail",
            "mode=warn",
            "harness=ok",
        ]
    );
}

#[test]
fn splits_failures_from_warnings_and_only_failures_are_unhealthy() {
    let warned = DoctorReport {
        checks: Vec::new(),
        findings: vec![finding(DoctorCheck::Mode, DoctorSeverity::Warn)],
    };
    let failed = DoctorReport {
        checks: Vec::new(),
        findings: vec![
            finding(DoctorCheck::Mode, DoctorSeverity::Warn),
            finding(DoctorCheck::Lock, DoctorSeverity::Fail),
        ],
    };

    assert!(is_healthy(&warned));
    assert!(!is_healthy(&failed));
    assert_eq!(doctor_failures(&failed).len(), 1);
    assert_eq!(doctor_warnings(&failed).len(), 1);
    assert_eq!(doctor_failures(&warned).len(), 0);
}

#[test]
fn lists_a_maps_strings_in_key_order() {
    let map: IndexMap<String, String> = [(s("b"), s("${B}")), (s("a"), s("${A}"))]
        .into_iter()
        .collect();

    assert_eq!(strings_in(&map), ["${A}", "${B}"]);
}

#[test]
fn keeps_a_mode_finding_quiet_when_nothing_is_installed() {
    let root = tempdir();

    assert_eq!(installed_mode(&root.path().join("absent")), None);
    assert_eq!(installed_mode(root.path()), Some(ArtifactMode::Copy));
}

#[test]
fn healthy_passes_every_check_and_names_them_rather_than_printing_nothing() {
    let fixture = installed();
    let result = fixture.cli(&["doctor"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(result.stdout, HEALTHY_REPORT);
}

#[test]
fn healthy_touches_nothing_so_it_can_run_on_a_project_it_reports_failures_on() {
    let fixture = installed();
    let before = fixture.snapshot();

    assert_eq!(fixture.cli(&["doctor"]).code, ExitCode::Success);
    fixture.stub_env(TAGGED_VAR, None);
    fs::remove_file(fixture.project_dir.join(LOCK_FILE)).expect("remove the lock");
    assert_eq!(fixture.cli(&["doctor"]).code, ExitCode::Doctor);

    let after = fixture.snapshot();

    assert_eq!(
        after.keys().cloned().collect::<Vec<_>>(),
        before
            .keys()
            .filter(|file| file.as_str() != LOCK_FILE)
            .cloned()
            .collect::<Vec<_>>()
    );
}

#[test]
fn healthy_emits_machine_readable_output_carrying_no_absolute_paths() {
    let fixture = installed();
    let result = fixture.cli(&["doctor", "--json"]);

    assert_eq!(
        serde_json::from_str::<Value>(&result.stdout).expect("JSON"),
        json!({
            "checks": [
                { "check": "expects", "status": "ok" },
                { "check": "lock", "status": "ok" },
                { "check": "ownership", "status": "ok" },
                { "check": "drift", "status": "ok" },
                { "check": "mode", "status": "ok" },
                { "check": "harness", "status": "ok" },
            ],
            "findings": [],
            "healthy": true,
        })
    );
    assert!(!result.stdout.contains(&*fixture.root.to_string_lossy()));
}

fn incomplete() -> Fixture {
    let fixture = Fixture::new();

    fixture.stub_env(FIGMA_VAR, None);
    fixture.stub_env(TAGGED_VAR, None);
    fixture.install(&[]);
    fixture
}

#[test]
fn incomplete_exits_6_reporting_one_failure_per_unset_variable_in_variable_order() {
    let fixture = incomplete();
    let result = fixture.cli(&["doctor"]);

    assert_eq!(result.code, ExitCode::Doctor);
    assert_eq!(result.stderr, "");
    assert_eq!(
        fixture.findings(),
        [
            format!("expects/fail: unset environment variable \"{FIGMA_VAR}\""),
            format!("expects/fail: unset environment variable \"{TAGGED_VAR}\""),
        ]
    );
    assert_eq!(
        fixture.checks(),
        [
            "expects=fail",
            "lock=ok",
            "ownership=ok",
            "drift=ok",
            "mode=ok",
            "harness=ok",
        ]
    );
}

#[test]
fn incomplete_names_the_skill_that_wants_the_variable_and_how_to_satisfy_it() {
    let fixture = incomplete();

    assert_eq!(
        fixture.detail_of(FIGMA_VAR),
        [
            format!("skill \"{FRONTEND_SKILL}\" expects it"),
            format!("set {FIGMA_VAR} in the environment the agent runs in"),
        ]
    );
}

#[test]
fn incomplete_names_the_server_and_the_reference_install_left_for_the_harness() {
    let fixture = incomplete();

    assert_eq!(
        fixture.detail_of(TAGGED_VAR),
        [
            s("MCP server \"linter\" expects it"),
            format!(
                "\"mcpServers.linter\" in {MCP_FILE} references it, for the harness to expand at spawn"
            ),
            format!("set {TAGGED_VAR} in the environment the agent runs in"),
        ]
    );
}

#[test]
fn incomplete_says_nothing_about_a_variable_set_to_the_empty_string() {
    let fixture = incomplete();

    fixture.stub_env(FIGMA_VAR, Some(""));

    assert_eq!(
        fixture.findings(),
        [format!(
            "expects/fail: unset environment variable \"{TAGGED_VAR}\""
        )]
    );
}

#[test]
fn incomplete_goes_quiet_once_the_variables_are_set_and_installed() {
    let fixture = incomplete();

    fixture.stub_env(FIGMA_VAR, Some("figma-token"));
    fixture.stub_env(TAGGED_VAR, Some("tagged-key"));
    fixture.install(&[]);

    assert!(is_healthy(&fixture.diagnose()));
}

const PLANNER: &str = "planner";
const PLANNER_VAR: &str = "ACME_PLANNER_TOKEN";

fn renaming() -> Fixture {
    let fixture = Fixture::new();

    write(
        &fixture
            .catalog_dir
            .join("mcps")
            .join(format!("{PLANNER}.yml")),
        &[
            format!("name: {PLANNER}"),
            s(""),
            s("transport:"),
            s("  stdio:"),
            s("    command: planner-mcp"),
            s("    env:"),
            format!("      PLANNER_TOKEN: \"${{{PLANNER_VAR}}}\""),
            s(""),
        ]
        .join("\n"),
    );
    write(
        &fixture.project_dir.join("ambit.yml"),
        &format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nrequires:\n  - {{ mcp: \"{CATALOG_NAME}/{PLANNER}\" }}\n"
        ),
    );
    fixture.stub_env(PLANNER_VAR, None);
    fixture.install(&[]);
    fixture
}

#[test]
fn renaming_asks_for_the_variable_the_file_references_not_the_name_the_process_reads() {
    let fixture = renaming();

    assert_eq!(
        fixture.findings(),
        [format!(
            "expects/fail: unset environment variable \"{PLANNER_VAR}\""
        )]
    );
    assert_eq!(
        fixture.detail_of(PLANNER_VAR),
        [
            format!(
                "\"mcpServers.{PLANNER}\" in {MCP_FILE} references it, for the harness to expand at spawn"
            ),
            format!("set {PLANNER_VAR} in the environment the agent runs in"),
        ]
    );
}

#[test]
fn renaming_goes_quiet_once_that_variable_is_set() {
    let fixture = renaming();

    fixture.stub_env(PLANNER_VAR, Some("planner-token"));

    assert!(is_healthy(&fixture.diagnose()));
}

#[test]
fn lock_resolution_would_rewrite_is_reported_naming_the_file() {
    let fixture = installed();

    write(&fixture.project_dir.join(LOCK_FILE), "version: 1\n");

    assert_eq!(
        fixture.findings(),
        [format!("lock/fail: {LOCK_FILE} is out of date")]
    );
    assert_eq!(fixture.cli(&["doctor"]).code, ExitCode::Doctor);
}

#[test]
fn lock_never_written_is_distinguished_from_one_that_is_stale() {
    let fixture = installed();

    fs::remove_file(fixture.project_dir.join(LOCK_FILE)).expect("remove the lock");

    assert_eq!(
        fixture.findings(),
        [format!("lock/fail: {LOCK_FILE} is missing")]
    );
    assert!(
        fixture
            .detail_of(LOCK_FILE)
            .contains(&s("run `ambit install` to write it"))
    );
}

#[test]
fn lock_is_quiet_after_a_prune_which_rewrites_it_along_with_state() {
    let fixture = installed();

    fixture.write_profile(&["core"]);
    assert_eq!(fixture.cli(&["prune"]).code, ExitCode::Success);

    assert_eq!(fixture.findings().len(), 0);
    assert_eq!(fixture.cli(&["doctor"]).code, ExitCode::Success);
}

#[test]
fn lock_names_the_two_commands_that_write_it() {
    let fixture = installed();

    write(&fixture.project_dir.join(LOCK_FILE), "version: 1\n");

    assert!(fixture.detail_of(LOCK_FILE).contains(&format!(
        "{LOCK_FILE} is written by `ambit install` and `ambit prune`, so config or a catalog commit has moved since the last one"
    )));
}

#[test]
fn ownership_reports_every_artifact_ambit_no_longer_owns_and_never_as_drift() {
    let fixture = installed();

    fs::remove_file(fixture.project_dir.join(STATE_FILE)).expect("remove state");

    assert_eq!(
        fixture.findings(),
        [
            format!("ownership/fail: ambit does not own {HOOK_TARGET}"),
            format!(
                "ownership/fail: ambit does not own {}",
                engineering_target()
            ),
            format!("ownership/fail: ambit does not own {}", core_target()),
            format!("ownership/fail: ambit does not own {}", frontend_target()),
            format!("ownership/fail: ambit does not own {CLAUDE_SETTINGS}"),
            format!("ownership/fail: ambit does not own {CLAUDE_LINK}"),
            format!("ownership/fail: ambit does not own {MCP_FILE}"),
        ]
    );
    assert!(fixture.checks().contains(&s("drift=ok")));
    assert_eq!(fixture.cli(&["doctor"]).code, ExitCode::Doctor);
}

#[test]
fn ownership_explains_the_crash_and_names_adopt() {
    let fixture = installed();

    fs::remove_file(fixture.project_dir.join(STATE_FILE)).expect("remove state");

    assert_eq!(
        fixture.detail_of(&core_target()),
        [
            "it exists but ambit did not create it",
            "an `ambit install` that crashed leaves this: state is written after the files it describes",
            "move it aside, or run `ambit install --adopt` to take ownership",
        ]
    );
}

#[test]
fn ownership_reports_a_co_owned_config_key_by_key() {
    let fixture = installed();

    fs::remove_file(fixture.project_dir.join(STATE_FILE)).expect("remove state");

    assert!(fixture.detail_of(MCP_FILE).contains(&s(
        "\"mcpServers.linter\" exists but ambit did not create it"
    )));
}

#[test]
fn drift_reports_a_deleted_skill_directory_with_statuss_own_detail() {
    let fixture = installed();

    fs::remove_dir_all(fixture.project_dir.join(engineering_target())).expect("remove the skill");

    assert_eq!(
        fixture.findings(),
        [format!("drift/fail: {} is missing", engineering_target())]
    );
    assert_eq!(
        fixture.detail_of(&engineering_target()),
        [
            "nothing is installed at this path",
            "run `ambit install` to write it"
        ]
    );
}

#[test]
fn drift_reports_the_managed_gitignore_block_which_status_has_no_row_for() {
    let fixture = installed();

    fs::remove_file(fixture.project_dir.join(GITIGNORE_FILE)).expect("remove .gitignore");

    assert_eq!(
        fixture.findings(),
        [format!(
            "drift/fail: {GITIGNORE_FILE} does not hold the block install would write"
        )]
    );
    assert_eq!(fixture.cli(&["doctor"]).code, ExitCode::Doctor);
}

#[test]
fn reports_every_failure_at_once_rather_than_stopping_at_the_first() {
    let fixture = installed();

    fixture.stub_env(TAGGED_VAR, None);
    crate::util::fs::rm_rf(&fixture.project_dir.join(core_target())).expect("remove the link");
    fs::remove_file(fixture.project_dir.join(LOCK_FILE)).expect("remove the lock");

    assert_eq!(
        fixture.findings(),
        [
            format!("expects/fail: unset environment variable \"{TAGGED_VAR}\""),
            format!("lock/fail: {LOCK_FILE} is missing"),
            format!("drift/fail: {} is missing", core_target()),
        ]
    );
}

fn copied() -> Fixture {
    let fixture = Fixture::new();

    fixture.stub_env(FIGMA_VAR, Some("figma-token"));
    fixture.stub_env(TAGGED_VAR, Some("tagged-key"));
    fixture.install(&["--copy"]);
    fixture
}

#[test]
fn copy_warns_that_a_plain_install_would_symlink_each_skill_and_still_exits_0() {
    let fixture = copied();
    let result = fixture.cli(&["doctor"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        fixture.findings(),
        [
            format!("mode/warn: {} is installed as a copy", engineering_target()),
            format!("mode/warn: {} is installed as a copy", core_target()),
            format!("mode/warn: {} is installed as a copy", frontend_target()),
            format!("mode/warn: {HOOK_TARGET} is installed as a copy"),
        ]
    );
    assert_eq!(
        fixture.checks(),
        [
            "expects=ok",
            "lock=ok",
            "ownership=ok",
            "drift=ok",
            "mode=warn",
            "harness=ok",
        ]
    );
}

#[test]
fn copy_says_why_and_how_to_keep_it() {
    let fixture = copied();

    assert_eq!(
        fixture.detail_of(&core_target()),
        [
            "its source is a local directory someone edits, so a plain `ambit install` would symlink it",
            "keep passing `--copy` to `ambit install` to leave it as it is",
        ]
    );
}

#[test]
fn copy_stays_silent_about_the_mode_of_a_skill_it_reports_as_modified() {
    let fixture = copied();

    write(
        &fixture.project_dir.join(core_target()).join("SKILL.md"),
        "edited by hand\n",
    );

    assert_eq!(
        fixture.findings(),
        [
            format!("drift/fail: {} is modified", core_target()),
            format!("mode/warn: {} is installed as a copy", engineering_target()),
            format!("mode/warn: {} is installed as a copy", frontend_target()),
            format!("mode/warn: {HOOK_TARGET} is installed as a copy"),
        ]
    );
}

fn hook_expecting() -> Fixture {
    let fixture = Fixture::new();
    let expects = format!("expects: [{{ env: {HOOK_VAR} }}]");
    let mut hook_lines = HOOK_LINES.to_vec();

    hook_lines.push(&expects);
    fixture.write_hook_profile(&["claude"], &hook_lines);
    fixture.stub_env(HOOK_VAR, None);
    fixture.install(&[]);
    fixture
}

#[test]
fn hook_expects_fails_on_a_variable_a_selected_hook_declares() {
    let fixture = hook_expecting();

    assert_eq!(fixture.cli(&["doctor"]).code, ExitCode::Doctor);
    assert_eq!(
        fixture.findings(),
        [format!(
            "expects/fail: unset environment variable \"{HOOK_VAR}\""
        )]
    );
    assert_eq!(
        fixture.detail_of(HOOK_VAR),
        [
            format!("hook \"{HOOK}\" expects it"),
            format!("set {HOOK_VAR} in the environment the agent runs in"),
        ]
    );
}

#[test]
fn hook_expects_goes_quiet_once_it_is_set_without_a_reinstall() {
    let fixture = hook_expecting();

    fixture.stub_env(HOOK_VAR, Some("https://hooks.example/notify"));

    assert!(is_healthy(&fixture.diagnose()));
}

#[test]
fn codex_warns_that_it_needs_the_feature_flag_and_still_exits_0() {
    let fixture = Fixture::new();

    fixture.write_hook_profile(&["codex"], HOOK_LINES);
    fixture.install(&[]);

    let result = fixture.cli(&["doctor"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        fixture.findings(),
        ["harness/warn: codex runs hooks only with `[features] codex_hooks = true` set"]
    );
    assert_eq!(
        fixture.checks(),
        [
            "expects=ok",
            "lock=ok",
            "ownership=ok",
            "drift=ok",
            "mode=ok",
            "harness=warn",
        ]
    );
}

#[test]
fn codex_names_the_file_ambit_wrote_and_says_the_flag_is_not_ambits_to_write() {
    let fixture = Fixture::new();

    fixture.write_hook_profile(&["codex"], HOOK_LINES);
    fixture.install(&[]);

    assert_eq!(
        fixture.detail_of("codex_hooks"),
        [
            "this project selects a hook, and ambit writes them to .codex/hooks.json",
            "codex's hooks are experimental, and the flag enabling them is user-level config ambit must not write",
            "set `[features] codex_hooks = true` in your own codex config to have them run",
        ]
    );
}

#[test]
fn codex_says_nothing_when_no_hook_is_selected() {
    let fixture = Fixture::new();

    fixture.write_hook_profile(&["codex"], &[]);
    fixture.install(&[]);

    assert_eq!(fixture.findings().len(), 0);
    assert!(fixture.checks().contains(&s("harness=ok")));
}

#[test]
fn codex_says_nothing_when_hooks_are_selected_and_codex_is_not_configured() {
    let fixture = Fixture::new();

    fixture.write_hook_profile(&["claude"], HOOK_LINES);
    fixture.install(&[]);

    assert_eq!(fixture.findings().len(), 0);
    assert_eq!(fixture.cli(&["doctor"]).code, ExitCode::Success);
}

#[test]
fn before_install_reports_the_missing_lock_and_every_missing_artifact() {
    let fixture = Fixture::new();

    fixture.stub_env(FIGMA_VAR, Some("figma-token"));
    fixture.stub_env(TAGGED_VAR, Some("tagged-key"));

    let result = fixture.cli(&["doctor"]);

    assert_eq!(result.code, ExitCode::Doctor);
    assert_eq!(
        fixture.checks(),
        [
            "expects=ok",
            "lock=fail",
            "ownership=ok",
            "drift=fail",
            "mode=ok",
            "harness=ok",
        ]
    );
    assert_eq!(doctor_failures(&fixture.diagnose()).len(), 10);
}

#[test]
fn before_install_a_broken_config_is_an_error_rather_than_a_finding() {
    let fixture = Fixture::new();

    fixture.write_profile(&["function.enginering"]);

    let result = fixture.cli(&["doctor"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stderr.contains(&format!(
        "`requires` entry \"pack:{CATALOG_NAME}/function.enginering\" matches nothing"
    )));
    assert_eq!(result.stdout, "");
}
