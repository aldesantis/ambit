use std::path::PathBuf;

use serde_json::json;

use crate::errors::ExitCode;
use crate::model::documents::{DocumentFormat, DocumentShape, array_entry_key, managed_key};
use crate::model::state::{
    ArtifactKind, OwnedArtifact, STATE_DIRNAME, STATE_FILENAME, parse_state,
};
use crate::test_support::{run_cli, tempdir, test_env};
use crate::util::env::Env;
use crate::util::fs::{EntryKind, lstat_kind, mkdir_p, read_text, rm_rf, write_text};
use crate::util::json::{JsonValue, parse, stringify_pretty};
use crate::util::path::{join, to_slash};

const SETTINGS: &str = ".claude/settings.json";

#[derive(Clone)]
struct Hook {
    name: String,
    lines: Vec<String>,
}

fn hook(name: &str, lines: &[&str]) -> Hook {
    Hook {
        name: name.to_owned(),
        lines: lines.iter().map(|&line| line.to_owned()).collect(),
    }
}

fn format_hook() -> Hook {
    hook(
        "format",
        &[
            "event: PostToolUse",
            "matcher: Write",
            "type: command",
            "command: npx prettier --write",
            "timeout: 30",
        ],
    )
}

fn notify_hook() -> Hook {
    hook(
        "notify",
        &["event: Stop", "type: command", "command: ./bin/notify"],
    )
}

fn format_entry() -> JsonValue {
    json!({
        "matcher": "Write",
        "hooks": [{ "type": "command", "command": "npx prettier --write", "timeout": 30 }],
    })
}

fn notify_entry() -> JsonValue {
    json!({ "hooks": [{ "type": "command", "command": "./bin/notify" }] })
}

fn format_key() -> String {
    managed_key("hooks", &array_entry_key("PostToolUse", &format_entry()))
}

fn notify_key() -> String {
    managed_key("hooks", &array_entry_key("Stop", &notify_entry()))
}

fn pretty(value: &JsonValue) -> String {
    format!("{}\n", stringify_pretty(value))
}

fn config_artifact(path: &str, keys: Vec<String>) -> OwnedArtifact {
    shaped_artifact(path, keys, Some(DocumentShape::Array))
}

fn shaped_artifact(path: &str, keys: Vec<String>, shape: Option<DocumentShape>) -> OwnedArtifact {
    OwnedArtifact {
        path: path.to_owned(),
        kind: ArtifactKind::HarnessConfig,
        mode: None,
        managed_keys: Some(keys),
        format: Some(DocumentFormat::Json),
        shape,
        digest: None,
    }
}

struct Output {
    code: ExitCode,
    stdout: String,
    stderr: String,
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    project_dir: PathBuf,
    install_dir: PathBuf,
    env: Env,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempdir();
        let root = tmp.path().to_path_buf();
        let project_dir = root.join("project");
        mkdir_p(&project_dir).unwrap();
        let env = test_env(&root);

        Self {
            _tmp: tmp,
            root,
            install_dir: project_dir.clone(),
            project_dir,
            env,
        }
    }

    fn user() -> Self {
        let mut f = Self::new();
        let home = f.root.join("home");
        f.project_dir = home.join(".ambit");
        mkdir_p(&f.project_dir).unwrap();
        f.env
            .insert("HOME".to_owned(), home.to_string_lossy().into_owned());
        f.install_dir = home;
        f
    }

    fn write_profile(&self, hooks: &[Hook], harnesses: &[&str]) {
        rm_rf(&self.project_dir.join("hooks")).unwrap();
        rm_rf(&self.project_dir.join("packs")).unwrap();

        for hook in hooks {
            let dir = self.project_dir.join("hooks").join(&hook.name);
            mkdir_p(&dir).unwrap();
            let mut lines = vec![format!("name: {}", hook.name)];
            lines.extend(hook.lines.iter().cloned());
            lines.push(String::new());
            write_text(&dir.join("hook.yml"), &lines.join("\n")).unwrap();
        }

        if !hooks.is_empty() {
            mkdir_p(&self.project_dir.join("packs")).unwrap();
            let mut lines = vec![
                "name: core".to_owned(),
                "description: The hooks this project ships.".to_owned(),
                "requires:".to_owned(),
            ];
            lines.extend(hooks.iter().map(|hook| format!("  - hook: {}", hook.name)));
            lines.push(String::new());
            write_text(&self.project_dir.join("packs/core.yml"), &lines.join("\n")).unwrap();
        }

        let requires = if hooks.is_empty() {
            "[]".to_owned()
        } else {
            format!("\n{}", requires_entry("core", "local"))
        };

        write_text(
            &self.project_dir.join("ambit.yml"),
            &format!(
                "version: 1\nharnesses: [{}]\ncatalogs:\n  - name: local\n    source: path:.\nrequires: {requires}\n",
                harnesses.join(", ")
            ),
        )
        .unwrap();
    }

    fn cli(&self, argv: &[&str]) -> Output {
        let project = self.project_dir.to_string_lossy().into_owned();
        let mut args: Vec<&str> = argv.to_vec();
        args.extend(["--project", &project]);
        let result = run_cli(&args, &self.root, &self.env);
        let joined = |text: String| text.strip_suffix('\n').unwrap_or(&text).to_owned();

        Output {
            code: result.code,
            stdout: joined(result.stdout),
            stderr: joined(result.stderr),
        }
    }

    fn file_text(&self, relative: &str) -> String {
        read_text(&join(&self.install_dir, relative)).unwrap()
    }

    fn settings_text(&self) -> String {
        self.file_text(SETTINGS)
    }

    fn settings(&self) -> JsonValue {
        parse(&self.settings_text()).unwrap()
    }

    fn file_json(&self, relative: &str) -> JsonValue {
        parse(&self.file_text(relative)).unwrap()
    }

    fn state_artifacts(&self) -> Vec<OwnedArtifact> {
        let text = read_text(&self.project_dir.join(STATE_DIRNAME).join(STATE_FILENAME)).unwrap();

        parse_state(&text, STATE_FILENAME).unwrap().artifacts
    }

    fn path_exists(&self, relative: &str) -> bool {
        !matches!(
            lstat_kind(&join(&self.install_dir, relative)),
            Ok(EntryKind::Missing) | Err(_)
        )
    }

    fn link_target(&self, relative: &str) -> Option<String> {
        std::fs::read_link(join(&self.install_dir, relative))
            .ok()
            .map(|target| to_slash(&target))
    }
}

fn requires_entry(pack: &str, catalog: &str) -> String {
    format!("  - {{ pack: \"{catalog}/{pack}\" }}")
}

mod claude_settings {
    use super::*;

    fn fixture() -> Fixture {
        let fixture = Fixture::new();
        fixture.write_profile(&[format_hook(), notify_hook()], &["claude"]);
        fixture
    }

    #[test]
    fn writes_one_entry_per_hook_and_records_each_entrys_digest_as_owned() {
        let f = fixture();
        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PostToolUse": [format_entry()], "Stop": [notify_entry()] } })
        );
        assert_eq!(
            f.state_artifacts(),
            [config_artifact(SETTINGS, vec![format_key(), notify_key()])]
        );
    }

    #[test]
    fn changes_no_bytes_on_a_second_install_and_reports_no_drift() {
        let f = fixture();
        f.cli(&["install"]);
        let written = f.settings_text();

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(f.settings_text(), written);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn prunes_the_entry_a_narrowed_config_no_longer_declares_leaving_the_array_behind() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&[format_hook()], &["claude"]);

        assert_eq!(f.cli(&["prune"]).code, ExitCode::Success);

        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PostToolUse": [format_entry()], "Stop": [] } })
        );
        assert_eq!(
            f.state_artifacts(),
            [config_artifact(SETTINGS, vec![format_key()])]
        );
    }

    #[test]
    fn writes_no_settings_file_at_all_for_a_project_that_declares_no_hooks() {
        let f = fixture();
        f.write_profile(&[], &["claude"]);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert!(!f.path_exists(SETTINGS));
    }
}

mod claude_and_the_harnesses_reading_its_file {
    use super::*;

    fn fixture() -> Fixture {
        let fixture = Fixture::new();
        fixture.write_profile(&[format_hook()], &["claude", "copilot", "devin", "grok"]);
        fixture
    }

    #[test]
    fn writes_the_shared_file_once_and_records_it_once() {
        let f = fixture();
        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PostToolUse": [format_entry()] } })
        );
        assert_eq!(
            f.state_artifacts(),
            [config_artifact(SETTINGS, vec![format_key()])]
        );
        assert_eq!(result.stdout.matches(SETTINGS).count(), 1);
    }

    #[test]
    fn leaves_their_own_configs_alone_having_nothing_to_put_in_them() {
        let f = fixture();
        f.cli(&["install"]);

        assert!(!f.path_exists(".vscode/mcp.json"));
        assert!(!f.path_exists(".devin"));
        assert!(!f.path_exists(".grok"));
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }
}

mod handwritten_settings {
    use super::*;

    const HANDWRITTEN: &str = r#"{
  "model": "opus",
  "permissions": {
    "allow": [
      "Bash(git status:*)"
    ]
  },
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "./bin/hello"
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "Write",
        "hooks": [
          {
            "type": "command",
            "command": "./bin/audit"
          }
        ]
      }
    ]
  }
}
"#;

    fn expected(mutate: impl FnOnce(&mut crate::util::json::JsonObject)) -> String {
        let mut document = parse(HANDWRITTEN).unwrap();
        let hooks = document["hooks"].as_object_mut().unwrap();

        mutate(hooks);

        pretty(&document)
    }

    fn fixture() -> Fixture {
        let f = Fixture::new();
        f.write_profile(&[format_hook(), notify_hook()], &["claude"]);
        mkdir_p(&f.project_dir.join(".claude")).unwrap();
        write_text(&join(&f.project_dir, SETTINGS), HANDWRITTEN).unwrap();
        f
    }

    #[test]
    fn survives_install_a_second_install_prune_and_clean_byte_identically() {
        let f = fixture();

        assert_eq!(expected(|_| {}), HANDWRITTEN);

        let installed = expected(|hooks| {
            hooks["PostToolUse"]
                .as_array_mut()
                .unwrap()
                .push(format_entry());
            hooks.insert("Stop".to_owned(), json!([notify_entry()]));
        });

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        assert_eq!(f.settings_text(), installed);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        assert_eq!(f.settings_text(), installed);

        assert_eq!(f.cli(&["prune"]).code, ExitCode::Success);
        assert_eq!(f.settings_text(), installed);

        assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);
        assert_eq!(
            f.settings_text(),
            expected(|hooks| {
                hooks.insert("Stop".to_owned(), json!([]));
            })
        );

        let mut remaining = f.settings();
        remaining["hooks"]
            .as_object_mut()
            .unwrap()
            .shift_remove("Stop");
        assert_eq!(pretty(&remaining), HANDWRITTEN);
    }

    #[test]
    fn never_claims_a_foreign_entry_whatever_event_it_sits_on() {
        let f = fixture();
        f.cli(&["install"]);

        let artifacts = f.state_artifacts();

        assert_eq!(
            artifacts[0].managed_keys,
            Some(vec![format_key(), notify_key()])
        );
    }
}

mod identical_handwritten_entry {
    use super::*;

    fn adopted() -> String {
        pretty(&json!({ "hooks": { "PostToolUse": [format_entry()] } }))
    }

    fn fixture() -> Fixture {
        let f = Fixture::new();
        f.write_profile(&[format_hook()], &["claude"]);
        mkdir_p(&f.project_dir.join(".claude")).unwrap();
        write_text(&join(&f.project_dir, SETTINGS), &adopted()).unwrap();
        f
    }

    #[test]
    fn refuses_it_by_name_leaving_the_project_untouched() {
        let f = fixture();
        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(result.stderr.contains(&format!(
            "\"{}\" in {SETTINGS} exists but ambit did not create",
            format_key()
        )));
        assert!(result.stderr.contains("ambit install --adopt"));
        assert_eq!(f.settings_text(), adopted());
        assert!(!f.path_exists(STATE_DIRNAME));
    }

    #[test]
    fn takes_it_over_under_adopt_without_writing_a_second_copy_of_it() {
        let f = fixture();
        let result = f.cli(&["install", "--adopt"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(f.settings_text(), adopted());
        assert_eq!(
            f.state_artifacts()[0].managed_keys,
            Some(vec![format_key()])
        );
    }

    #[test]
    fn prunes_what_it_adopted_once_the_declaration_is_gone() {
        let f = fixture();
        f.cli(&["install", "--adopt"]);
        f.write_profile(&[], &["claude"]);

        assert_eq!(f.cli(&["prune"]).code, ExitCode::Success);

        assert_eq!(f.settings(), json!({ "hooks": { "PostToolUse": [] } }));
        assert_eq!(f.state_artifacts(), []);
    }
}

mod digest_mismatch {
    use super::*;

    fn retimed_hook() -> Hook {
        hook(
            "format",
            &[
                "event: PostToolUse",
                "matcher: Write",
                "type: command",
                "command: npx prettier --write",
                "timeout: 45",
            ],
        )
    }

    fn retimed_entry() -> JsonValue {
        json!({
            "matcher": "Write",
            "hooks": [{ "type": "command", "command": "npx prettier --write", "timeout": 45 }],
        })
    }

    fn retimed_key() -> String {
        managed_key("hooks", &array_entry_key("PostToolUse", &retimed_entry()))
    }

    fn edited_entry() -> JsonValue {
        json!({
            "matcher": "Write",
            "hooks": [{ "type": "command", "command": "npx prettier --write", "timeout": 60 }],
        })
    }

    fn edit_installed_entry(f: &Fixture) {
        let text = f.settings_text();

        assert!(text.contains("\"timeout\": 30"));
        write_text(
            &join(&f.project_dir, SETTINGS),
            &text.replace("\"timeout\": 30", "\"timeout\": 60"),
        )
        .unwrap();
    }

    fn status_rows(f: &Fixture) -> JsonValue {
        let result = f.cli(&["status", "--json"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        parse(&result.stdout).unwrap()["artifacts"].clone()
    }

    fn fixture() -> Fixture {
        let f = Fixture::new();
        f.write_profile(&[format_hook()], &["claude"]);
        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        f
    }

    #[test]
    fn reports_the_hand_edited_entry_as_drift_naming_the_digest_state_claims() {
        let f = fixture();
        edit_installed_entry(&f);

        assert_eq!(
            status_rows(&f),
            json!([{
                "detail": format!("\"{}\" is absent", format_key()),
                "kind": "harness-config",
                "path": SETTINGS,
                "state": "missing",
            }])
        );
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Drift);
    }

    #[test]
    fn puts_its_own_entry_back_on_the_next_install_and_leaves_the_edit_as_the_persons() {
        let f = fixture();
        edit_installed_entry(&f);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PostToolUse": [edited_entry(), format_entry()] } })
        );
        assert_eq!(
            f.state_artifacts()[0].managed_keys,
            Some(vec![format_key()])
        );

        let healed = f.settings_text();

        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        assert_eq!(f.settings_text(), healed);
    }

    #[test]
    fn reports_the_digest_a_changed_declaration_now_wants_as_missing() {
        let f = fixture();
        f.write_profile(&[retimed_hook()], &["claude"]);

        assert_eq!(
            status_rows(&f),
            json!([{
                "detail": format!("\"{}\" is absent", retimed_key()),
                "kind": "harness-config",
                "path": SETTINGS,
                "state": "missing",
            }])
        );
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Drift);
    }

    #[test]
    fn prunes_the_stale_digest_and_writes_the_current_one_leaving_no_duplicate() {
        let f = fixture();
        f.write_profile(&[retimed_hook()], &["claude"]);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PostToolUse": [retimed_entry()] } })
        );
        assert_eq!(
            f.state_artifacts()[0].managed_keys,
            Some(vec![retimed_key()])
        );
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }
}

mod cursor_hooks {
    use super::*;

    const HOOKS_JSON: &str = ".cursor/hooks.json";

    fn watch_entry() -> JsonValue {
        json!({ "command": "./bin/watch" })
    }

    fn write_watch_profile(f: &Fixture, event: &str, harnesses: &[&str]) {
        f.write_profile(
            &[hook(
                "watch",
                &[
                    &format!("event: {event}"),
                    "type: command",
                    "command: ./bin/watch",
                ],
            )],
            harnesses,
        );
    }

    const EVENTS: &[(&str, &str)] = &[
        ("SessionStart", "sessionStart"),
        ("UserPromptSubmit", "userPromptSubmit"),
        ("PreToolUse", "preToolUse"),
        ("PostToolUse", "postToolUse"),
        ("Stop", "stop"),
        ("SubagentStop", "subagentStop"),
        ("PreCompact", "preCompact"),
        ("SessionEnd", "sessionEnd"),
    ];

    #[test]
    fn puts_each_event_in_cursors_own_array() {
        for &(event, spelling) in EVENTS {
            let f = Fixture::new();
            write_watch_profile(&f, event, &["cursor"]);

            let result = f.cli(&["install"]);

            assert_eq!(result.code, ExitCode::Success, "{event}: {}", result.stderr);

            let mut hooks = crate::util::json::JsonObject::new();
            hooks.insert(spelling.to_owned(), json!([watch_entry()]));
            assert_eq!(
                f.file_text(HOOKS_JSON),
                pretty(&json!({ "version": 1, "hooks": hooks })),
                "{event}"
            );
            assert_eq!(
                f.state_artifacts(),
                [config_artifact(
                    HOOKS_JSON,
                    vec![managed_key(
                        "hooks",
                        &array_entry_key(spelling, &watch_entry())
                    )],
                )],
                "{event}"
            );
        }
    }

    #[test]
    fn changes_no_bytes_on_a_second_install_and_reports_no_drift() {
        let f = Fixture::new();
        write_watch_profile(&f, "PreCompact", &["cursor"]);
        f.cli(&["install"]);
        let written = f.file_text(HOOKS_JSON);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(f.file_text(HOOKS_JSON), written);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn drops_a_matcher_which_cursor_has_no_field_for() {
        let f = Fixture::new();
        f.write_profile(
            &[hook(
                "guard",
                &[
                    "event: PreToolUse",
                    "matcher: Bash",
                    "type: command",
                    "command: ./bin/guard",
                ],
            )],
            &["cursor"],
        );

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        let written = f.file_text(HOOKS_JSON);

        assert_eq!(
            parse(&written).unwrap(),
            json!({ "version": 1, "hooks": { "preToolUse": [{ "command": "./bin/guard" }] } })
        );
        assert!(!written.contains("Bash"));
        assert!(!written.contains("matcher"));
    }

    #[test]
    fn leaves_the_whole_document_alone_but_for_the_array_it_appends_to() {
        let f = Fixture::new();
        let handwritten =
            pretty(&json!({ "version": 2, "hooks": { "stop": [{ "command": "./bin/mine" }] } }));

        write_watch_profile(&f, "Stop", &["cursor"]);
        mkdir_p(&f.project_dir.join(".cursor")).unwrap();
        write_text(&join(&f.project_dir, HOOKS_JSON), &handwritten).unwrap();

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        let installed = pretty(&json!({
            "version": 2,
            "hooks": { "stop": [{ "command": "./bin/mine" }, watch_entry()] },
        }));

        assert_eq!(f.file_text(HOOKS_JSON), installed);

        assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);
        assert_eq!(f.file_text(HOOKS_JSON), handwritten);
    }
}

mod codex_hooks {
    use super::*;

    const CODEX_HOOKS: &str = ".codex/hooks.json";

    fn fixture() -> Fixture {
        let f = Fixture::new();
        f.write_profile(&[format_hook(), notify_hook()], &["codex"]);
        f
    }

    #[test]
    fn writes_claudes_own_entries_under_claudes_own_event_names() {
        let f = fixture();
        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(
            f.file_text(CODEX_HOOKS),
            pretty(&json!({
                "hooks": { "PostToolUse": [format_entry()], "Stop": [notify_entry()] },
            }))
        );
        assert_eq!(
            f.state_artifacts(),
            [config_artifact(
                CODEX_HOOKS,
                vec![format_key(), notify_key()]
            )]
        );
    }

    #[test]
    fn touches_no_config_toml_which_is_the_file_this_layout_exists_to_avoid() {
        let f = fixture();
        f.cli(&["install"]);

        assert!(!f.path_exists(".codex/config.toml"));
    }

    #[test]
    fn changes_no_bytes_on_a_second_install_and_reports_no_drift() {
        let f = fixture();
        f.cli(&["install"]);
        let written = f.file_text(CODEX_HOOKS);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(f.file_text(CODEX_HOOKS), written);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn leaves_a_hook_of_someone_elses_where_it_is_and_gives_it_back_on_clean() {
        let f = fixture();
        let mine = json!({ "hooks": [{ "type": "command", "command": "./bin/mine" }] });
        let handwritten = pretty(&json!({ "hooks": { "Stop": [mine.clone()] } }));

        mkdir_p(&f.project_dir.join(".codex")).unwrap();
        write_text(&join(&f.project_dir, CODEX_HOOKS), &handwritten).unwrap();

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(
            f.file_json(CODEX_HOOKS),
            json!({
                "hooks": {
                    "Stop": [mine.clone(), notify_entry()],
                    "PostToolUse": [format_entry()],
                },
            })
        );

        assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);
        assert_eq!(
            f.file_json(CODEX_HOOKS),
            json!({ "hooks": { "Stop": [mine], "PostToolUse": [] } })
        );
    }

    #[test]
    fn writes_each_of_claude_and_codex_its_own_file_from_one_rendering() {
        let f = fixture();
        f.write_profile(&[notify_hook()], &["claude", "codex"]);

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(
            f.settings(),
            json!({ "hooks": { "Stop": [notify_entry()] } })
        );
        assert_eq!(
            f.file_json(CODEX_HOOKS),
            json!({ "hooks": { "Stop": [notify_entry()] } })
        );
        let paths: Vec<String> = f
            .state_artifacts()
            .into_iter()
            .map(|artifact| artifact.path)
            .collect();
        assert_eq!(paths, [SETTINGS, CODEX_HOOKS]);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }
}

mod gemini_hooks {
    use super::*;

    const GEMINI: &str = ".gemini/settings.json";

    fn format_gemini() -> JsonValue {
        json!({
            "matcher": "write_file",
            "hooks": [{
                "name": "format",
                "type": "command",
                "command": "npx prettier --write",
                "timeout": 30000,
            }],
        })
    }

    fn notify_gemini() -> JsonValue {
        json!({ "hooks": [{ "name": "notify", "type": "command", "command": "./bin/notify" }] })
    }

    fn hook_keys() -> Vec<String> {
        vec![
            managed_key("hooks", &array_entry_key("AfterAgent", &notify_gemini())),
            managed_key("hooks", &array_entry_key("AfterTool", &format_gemini())),
        ]
    }

    fn write_with_server(f: &Fixture, hooks: &[Hook]) {
        f.write_profile(hooks, &["gemini"]);
        mkdir_p(&f.project_dir.join("mcps")).unwrap();
        write_text(
            &f.project_dir.join("mcps/docs.yml"),
            "name: docs

transport:
  http:
    url: https://mcp.invalid/docs
",
        )
        .unwrap();

        let mut lines = vec![
            "name: core".to_owned(),
            "description: The hooks and server this project ships.".to_owned(),
            "requires:".to_owned(),
            "  - mcp: docs".to_owned(),
        ];
        lines.extend(hooks.iter().map(|hook| format!("  - hook: {}", hook.name)));
        lines.push(String::new());
        mkdir_p(&f.project_dir.join("packs")).unwrap();
        write_text(
            &f.project_dir.join("packs/core.yml"),
            &lines.join(
                "
",
            ),
        )
        .unwrap();
        write_text(
            &f.project_dir.join("ambit.yml"),
            &format!(
                "version: 1\nharnesses: [gemini]\ncatalogs:\n  - name: local\n    source: path:.\nrequires:\n{}\n",
                requires_entry("core", "local")
            ),
        )
        .unwrap();
    }

    fn fixture() -> Fixture {
        let f = Fixture::new();
        write_with_server(&f, &[format_hook(), notify_hook()]);
        f
    }

    #[test]
    fn writes_servers_and_hooks_side_by_side_and_records_each_section_on_its_own() {
        let f = fixture();
        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(
            f.file_text(GEMINI),
            pretty(&json!({
                "mcpServers": { "docs": { "httpUrl": "https://mcp.invalid/docs" } },
                "hooks": { "AfterTool": [format_gemini()], "AfterAgent": [notify_gemini()] },
            }))
        );
        assert_eq!(
            f.state_artifacts(),
            [
                shaped_artifact(GEMINI, vec![managed_key("mcpServers", "docs")], None),
                config_artifact(GEMINI, hook_keys()),
            ]
        );
    }

    #[test]
    fn changes_no_bytes_on_a_second_install_and_reports_no_drift() {
        let f = fixture();
        f.cli(&["install"]);
        let written = f.file_text(GEMINI);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(f.file_text(GEMINI), written);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn prunes_the_hooks_and_keeps_the_server_once_the_hooks_are_gone() {
        let f = fixture();
        f.cli(&["install"]);
        write_with_server(&f, &[]);

        let result = f.cli(&["prune"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(
            f.file_json(GEMINI),
            json!({
                "mcpServers": { "docs": { "httpUrl": "https://mcp.invalid/docs" } },
                "hooks": { "AfterTool": [], "AfterAgent": [] },
            })
        );
        assert_eq!(
            f.state_artifacts(),
            [shaped_artifact(
                GEMINI,
                vec![managed_key("mcpServers", "docs")],
                None
            )]
        );
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn prunes_both_sections_at_once_and_stops_claiming_either() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&[], &["gemini"]);

        let result = f.cli(&["prune"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(
            f.file_json(GEMINI),
            json!({ "mcpServers": {}, "hooks": { "AfterTool": [], "AfterAgent": [] } })
        );
        assert!(
            f.state_artifacts()
                .iter()
                .all(|artifact| artifact.path != GEMINI)
        );
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn takes_both_sections_back_on_clean() {
        let f = fixture();
        f.cli(&["install"]);

        assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);

        assert_eq!(
            f.file_json(GEMINI),
            json!({ "mcpServers": {}, "hooks": { "AfterTool": [], "AfterAgent": [] } })
        );
    }

    #[test]
    fn warns_for_a_subagent_hook_which_gemini_has_no_event_for() {
        let f = Fixture::new();
        f.write_profile(
            &[hook(
                "subagent",
                &[
                    "event: SubagentStop",
                    "type: command",
                    "command: ./bin/done",
                ],
            )],
            &["gemini"],
        );

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success);
        assert!(
            result
                .stderr
                .contains("gemini has no spelling for the SubagentStop event"),
            "{}",
            result.stderr
        );
        assert!(!f.path_exists(GEMINI));
    }
}

mod kiro_hooks {
    use super::*;

    const KIRO: &str = ".kiro/hooks/ambit.json";

    fn format_kiro() -> JsonValue {
        json!({
            "name": "format",
            "trigger": "PostToolUse",
            "matcher": "Write",
            "action": { "type": "command", "command": "npx prettier --write" },
            "timeout": 30,
        })
    }

    fn notify_kiro() -> JsonValue {
        json!({
            "name": "notify",
            "trigger": "Stop",
            "action": { "type": "command", "command": "./bin/notify" },
        })
    }

    fn fixture() -> Fixture {
        let f = Fixture::new();
        f.write_profile(&[format_hook(), notify_hook()], &["kiro"]);
        f
    }

    #[test]
    fn writes_one_list_with_a_version_and_records_each_entry_by_trigger_and_digest() {
        let f = fixture();
        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(
            f.file_text(KIRO),
            pretty(&json!({ "version": "v1", "hooks": [format_kiro(), notify_kiro()] }))
        );
        assert_eq!(
            f.state_artifacts(),
            [shaped_artifact(
                KIRO,
                vec![
                    managed_key("hooks", &array_entry_key("PostToolUse", &format_kiro())),
                    managed_key("hooks", &array_entry_key("Stop", &notify_kiro())),
                ],
                Some(DocumentShape::List)
            )]
        );
    }

    #[test]
    fn changes_no_bytes_on_a_second_install_and_reports_no_drift() {
        let f = fixture();
        f.cli(&["install"]);
        let written = f.file_text(KIRO);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(f.file_text(KIRO), written);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn reports_a_hand_edited_entry_as_missing() {
        let f = fixture();
        f.cli(&["install"]);
        let edited = f.file_text(KIRO).replacen("./bin/notify", "./bin/other", 1);
        write_text(&join(&f.project_dir, KIRO), &edited).unwrap();

        assert_ne!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn prunes_the_entry_a_narrowed_config_no_longer_declares() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&[format_hook()], &["kiro"]);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(
            f.file_json(KIRO),
            json!({ "version": "v1", "hooks": [format_kiro()] })
        );
    }

    #[test]
    fn leaves_a_hook_of_someone_elses_where_it_is_and_gives_it_back_on_clean() {
        let f = fixture();
        let mine = json!({
            "name": "mine",
            "trigger": "Stop",
            "action": { "type": "command", "command": "./bin/mine" },
        });
        let handwritten = pretty(&json!({ "version": "v1", "hooks": [mine.clone()] }));

        mkdir_p(&f.project_dir.join(".kiro/hooks")).unwrap();
        write_text(&join(&f.project_dir, KIRO), &handwritten).unwrap();

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        assert_eq!(
            f.file_json(KIRO),
            json!({ "version": "v1", "hooks": [mine.clone(), format_kiro(), notify_kiro()] })
        );

        assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);
        assert_eq!(f.file_text(KIRO), handwritten);
    }

    #[test]
    fn warns_for_the_events_kiro_has_no_trigger_for() {
        let f = Fixture::new();
        f.write_profile(
            &[hook(
                "compact",
                &[
                    "event: PreCompact",
                    "type: command",
                    "command: ./bin/compact",
                ],
            )],
            &["kiro"],
        );

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success);
        assert!(
            result
                .stderr
                .contains("kiro has no spelling for the PreCompact event"),
            "{}",
            result.stderr
        );
        assert!(!f.path_exists(KIRO));
    }
}

mod opencode_skip {
    use super::*;

    #[test]
    fn warns_exits_0_and_writes_opencode_nothing() {
        let f = Fixture::new();
        f.write_profile(&[notify_hook()], &["opencode"]);

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success);
        assert_eq!(
            result.stderr,
            "warning: hook \"notify\" (Stop) not installed: opencode has no declarative hook mechanism"
        );
        assert!(!f.path_exists(".opencode/opencode.jsonc"));
        assert!(!f.path_exists(SETTINGS));
        assert_eq!(f.state_artifacts(), []);
    }

    #[test]
    fn installs_the_hook_everywhere_else_and_warns_only_for_opencode() {
        let f = Fixture::new();
        f.write_profile(&[notify_hook()], &["claude", "opencode"]);

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success);

        assert_eq!(
            f.settings(),
            json!({ "hooks": { "Stop": [notify_entry()] } })
        );
        assert!(
            result
                .stderr
                .contains("opencode has no declarative hook mechanism")
        );
        assert_eq!(result.stderr.split('\n').count(), 1);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn says_the_same_thing_on_a_dry_run_having_written_nothing() {
        let f = Fixture::new();
        f.write_profile(&[notify_hook()], &["opencode"]);

        let result = f.cli(&["install", "--dry-run"]);

        assert_eq!(result.code, ExitCode::Success);
        assert!(
            result
                .stderr
                .contains("hook \"notify\" (Stop) not installed")
        );
        assert!(!f.path_exists(STATE_DIRNAME));
    }

    #[test]
    fn carries_the_skip_in_json_as_the_reason_rather_than_the_sentence() {
        let f = Fixture::new();
        f.write_profile(&[notify_hook()], &["opencode"]);

        let result = f.cli(&["install", "--json"]);

        assert_eq!(result.code, ExitCode::Success);

        assert_eq!(
            parse(&result.stdout).unwrap()["skipped"],
            json!([{ "event": "Stop", "harness": "opencode", "hook": "notify", "reason": "no-mechanism" }])
        );
        assert!(result.stderr.contains("no declarative hook mechanism"));
    }
}

mod claude_and_cursor {
    use super::*;

    #[test]
    fn writes_each_harness_its_own_file_in_that_harnesss_own_shape() {
        let f = Fixture::new();
        f.write_profile(&[notify_hook()], &["claude", "cursor"]);

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(
            f.settings(),
            json!({ "hooks": { "Stop": [notify_entry()] } })
        );
        assert_eq!(
            f.file_json(".cursor/hooks.json"),
            json!({ "version": 1, "hooks": { "stop": [{ "command": "./bin/notify" }] } })
        );
        let paths: Vec<String> = f
            .state_artifacts()
            .into_iter()
            .map(|artifact| artifact.path)
            .collect();
        assert_eq!(paths, [SETTINGS, ".cursor/hooks.json"]);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }
}

mod script_hook {
    use super::*;
    use crate::project::doctor::{DoctorOptions, diagnose_project};
    use crate::project::gitignore::{BLOCK_BEGIN, BLOCK_END, SHARED_GITIGNORE_FILE};
    use crate::util::cmp::js_cmp;

    const SCRIPT_HOOK: &str = "block-rm";
    const HOOK_DIR: &str = ".agents/hooks/block-rm";
    const SCRIPT: &str = "hook.sh";
    const SCRIPT_BODY: &str = "#!/bin/sh\nexit 0\n";

    fn claude_command() -> String {
        format!("${{CLAUDE_PROJECT_DIR}}/{HOOK_DIR}/{SCRIPT}")
    }

    fn relative_command() -> String {
        format!("{HOOK_DIR}/{SCRIPT}")
    }

    fn script_entry() -> JsonValue {
        json!({ "matcher": "Bash", "hooks": [{ "type": "command", "command": claude_command() }] })
    }

    fn announce_entry() -> JsonValue {
        json!({ "hooks": [{ "type": "command", "command": "npx --yes say done" }] })
    }

    fn hook_keys() -> Vec<String> {
        let mut keys = vec![
            managed_key("hooks", &array_entry_key("PreToolUse", &script_entry())),
            managed_key("hooks", &array_entry_key("Stop", &announce_entry())),
        ];
        keys.sort_by(|a, b| js_cmp(a, b));
        keys
    }

    fn write_catalog(f: &Fixture, harnesses: &[&str]) {
        let catalog_dir = f.project_dir.parent().unwrap().join("catalog");
        let files: [(String, String); 4] = [
            (
                "packs/core.yml".to_owned(),
                [
                    "name: core",
                    "description: The hooks this catalog ships.",
                    "requires:",
                    &format!("  - hook: {SCRIPT_HOOK}"),
                    "  - hook: announce",
                    "",
                ]
                .join("\n"),
            ),
            (
                format!("hooks/{SCRIPT_HOOK}/hook.yml"),
                [
                    &format!("name: {SCRIPT_HOOK}"),
                    "event: PreToolUse",
                    "matcher: Bash",
                    "type: script",
                    &format!("command: {SCRIPT}"),
                    "",
                ]
                .join("\n"),
            ),
            (
                format!("hooks/{SCRIPT_HOOK}/{SCRIPT}"),
                SCRIPT_BODY.to_owned(),
            ),
            (
                "hooks/announce/hook.yml".to_owned(),
                [
                    "name: announce",
                    "event: Stop",
                    "type: command",
                    "command: npx --yes say done",
                    "",
                ]
                .join("\n"),
            ),
        ];

        for (relative, body) in &files {
            let target = join(&catalog_dir, relative);
            mkdir_p(target.parent().unwrap()).unwrap();
            write_text(&target, body).unwrap();
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(
                join(&catalog_dir, &format!("hooks/{SCRIPT_HOOK}/{SCRIPT}")),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
        }

        write_text(
            &f.project_dir.join("ambit.yml"),
            &format!(
                "version: 1\nharnesses: [{}]\ncatalogs:\n  - name: company\n    source: path:../catalog\nrequires:\n{}\n",
                harnesses.join(", "),
                requires_entry("core", "company")
            ),
        )
        .unwrap();
    }

    fn fixture() -> Fixture {
        let f = Fixture::new();
        write_catalog(&f, &["claude"]);
        f
    }

    fn status_artifacts(f: &Fixture) -> Vec<JsonValue> {
        let result = f.cli(&["status", "--json"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        parse(&result.stdout).unwrap()["artifacts"]
            .as_array()
            .unwrap()
            .clone()
    }

    #[test]
    fn materializes_the_hooks_directory_and_records_it_as_a_hook_dir() {
        let f = fixture();
        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(f.file_text(&format!("{HOOK_DIR}/{SCRIPT}")), SCRIPT_BODY);
        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PreToolUse": [script_entry()], "Stop": [announce_entry()] } })
        );

        assert_eq!(
            f.state_artifacts(),
            [
                OwnedArtifact {
                    path: HOOK_DIR.to_owned(),
                    kind: ArtifactKind::HookDir,
                    mode: Some(crate::model::state::ArtifactMode::Link),
                    managed_keys: None,
                    format: None,
                    shape: None,
                    digest: None,
                },
                config_artifact(SETTINGS, hook_keys()),
            ]
        );
    }

    #[test]
    fn plans_no_directory_for_the_command_line_hook_beside_it() {
        let f = fixture();
        f.cli(&["install"]);

        assert!(!f.path_exists(".agents/hooks/announce"));
        assert_eq!(
            f.state_artifacts()
                .iter()
                .filter(|artifact| artifact.kind == ArtifactKind::HookDir)
                .count(),
            1
        );
    }

    #[test]
    fn writes_the_materialized_path_the_way_each_harness_resolves_one() {
        let f = fixture();
        write_catalog(
            &f,
            &["claude", "codex", "copilot", "cursor", "gemini", "kiro"],
        );

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert_eq!(f.file_text(&format!("{HOOK_DIR}/{SCRIPT}")), SCRIPT_BODY);

        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PreToolUse": [script_entry()], "Stop": [announce_entry()] } })
        );
        assert!(f.settings_text().contains(&claude_command()));

        assert_eq!(
            f.file_json(".codex/hooks.json"),
            json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "Bash",
                        "hooks": [{ "type": "command", "command": relative_command() }],
                    }],
                    "Stop": [announce_entry()],
                },
            })
        );

        assert_eq!(
            f.file_json(".cursor/hooks.json"),
            json!({
                "version": 1,
                "hooks": {
                    "preToolUse": [{ "command": relative_command() }],
                    "stop": [{ "command": "npx --yes say done" }],
                },
            })
        );

        assert_eq!(
            f.file_json(".gemini/settings.json"),
            json!({
                "hooks": {
                    "BeforeTool": [{
                        "matcher": "run_shell_command",
                        "hooks": [{
                            "name": SCRIPT_HOOK,
                            "type": "command",
                            "command": format!("$GEMINI_PROJECT_DIR/{HOOK_DIR}/{SCRIPT}"),
                        }],
                    }],
                    "AfterAgent": [{
                        "hooks": [{
                            "name": "announce",
                            "type": "command",
                            "command": "npx --yes say done",
                        }],
                    }],
                },
            })
        );

        assert_eq!(
            f.file_json(".kiro/hooks/ambit.json"),
            json!({
                "version": "v1",
                "hooks": [
                    {
                        "name": "announce",
                        "trigger": "Stop",
                        "action": { "type": "command", "command": "npx --yes say done" },
                    },
                    {
                        "name": SCRIPT_HOOK,
                        "trigger": "PreToolUse",
                        "matcher": "Bash",
                        "action": { "type": "command", "command": relative_command() },
                    },
                ],
            })
        );

        for file in [
            SETTINGS,
            ".codex/hooks.json",
            ".cursor/hooks.json",
            ".gemini/settings.json",
            ".kiro/hooks/ambit.json",
        ] {
            assert!(f.file_text(file).contains("npx --yes say done"), "{file}");
            assert!(!f.file_text(file).contains("hooks/announce"), "{file}");
        }
    }

    #[test]
    fn keeps_the_directory_on_a_second_install_rather_than_deleting_and_rewriting_it() {
        let f = fixture();
        f.cli(&["install"]);
        let written = f.settings_text();

        let preview = f.cli(&["install", "--dry-run", "--json"]);

        assert_eq!(preview.code, ExitCode::Success, "{}", preview.stderr);
        assert_eq!(parse(&preview.stdout).unwrap()["pruned"], json!([]));

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        assert_eq!(f.file_text(&format!("{HOOK_DIR}/{SCRIPT}")), SCRIPT_BODY);
        assert_eq!(f.settings_text(), written);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn reports_the_directory_as_a_hook_dir_compared_as_a_directory() {
        let f = fixture();
        f.cli(&["install"]);

        let artifacts = status_artifacts(&f);

        assert!(
            artifacts.contains(&json!({ "kind": "hook-dir", "path": HOOK_DIR, "state": "ok" }))
        );
    }

    #[test]
    fn reports_an_edited_script_as_modified_naming_the_file() {
        let f = fixture();
        f.cli(&["install", "--copy"]);
        write_text(
            &join(&f.project_dir, &format!("{HOOK_DIR}/{SCRIPT}")),
            "#!/bin/sh\nexit 1\n",
        )
        .unwrap();

        let artifacts = status_artifacts(&f);

        assert!(artifacts.contains(&json!({
            "detail": format!("{SCRIPT} differs from its source"),
            "kind": "hook-dir",
            "path": HOOK_DIR,
            "state": "modified",
        })));
    }

    #[test]
    fn writes_the_scripts_bytes_in_both_materialization_modes() {
        let f = fixture();
        f.cli(&["install"]);
        assert!(
            f.link_target(HOOK_DIR)
                .is_some_and(|target| target.contains(&format!("hooks/{SCRIPT_HOOK}")))
        );
        assert_eq!(f.file_text(&format!("{HOOK_DIR}/{SCRIPT}")), SCRIPT_BODY);

        assert_eq!(f.cli(&["install", "--copy"]).code, ExitCode::Success);

        assert_eq!(f.link_target(HOOK_DIR), None);
        assert_eq!(f.file_text(&format!("{HOOK_DIR}/{SCRIPT}")), SCRIPT_BODY);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_script_executable_through_a_copy_install() {
        use std::os::unix::fs::PermissionsExt;

        let f = fixture();
        assert_eq!(f.cli(&["install", "--copy"]).code, ExitCode::Success);

        let mode = std::fs::symlink_metadata(join(&f.project_dir, &format!("{HOOK_DIR}/{SCRIPT}")))
            .unwrap()
            .permissions()
            .mode();

        assert_eq!(mode & 0o111, 0o111);
    }

    #[test]
    fn lists_the_directory_in_the_volatile_agents_gitignore_block() {
        let f = fixture();
        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        let text = f.file_text(SHARED_GITIGNORE_FILE);
        let lines: Vec<&str> = text.split('\n').collect();
        let start = lines
            .iter()
            .position(|line| line.starts_with(BLOCK_BEGIN))
            .expect("a managed block");
        let end = lines
            .iter()
            .position(|line| line.starts_with(BLOCK_END))
            .expect("a block end");

        assert_eq!(lines[start + 1..end], [format!("/hooks/{SCRIPT_HOOK}")]);
    }

    #[test]
    fn reports_the_directorys_mode_divergence_in_doctor() {
        let f = fixture();
        f.cli(&["install", "--copy"]);

        let report = diagnose_project(&f.project_dir, &f.env, DoctorOptions::default()).unwrap();
        let found: Vec<String> = report
            .findings
            .iter()
            .filter(|finding| finding.message.contains(HOOK_DIR))
            .map(|finding| {
                format!(
                    "{}/{}: {}",
                    finding.check, finding.severity, finding.message
                )
            })
            .collect();

        assert_eq!(
            found,
            [format!("mode/warn: {HOOK_DIR} is installed as a copy")]
        );
    }

    #[test]
    fn takes_the_directory_back_on_clean_and_leaves_the_catalogs_copy_alone() {
        let f = fixture();
        f.cli(&["install", "--copy"]);

        assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);

        assert!(!f.path_exists(HOOK_DIR));
        assert_eq!(
            read_text(&join(
                &f.root.join("catalog"),
                &format!("hooks/{SCRIPT_HOOK}/{SCRIPT}")
            ))
            .unwrap(),
            SCRIPT_BODY
        );
    }

    #[test]
    fn stops_materializing_the_directory_once_the_hook_leaves_the_bundle() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&[], &["claude"]);

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        assert!(!f.path_exists(HOOK_DIR));
        assert_eq!(f.state_artifacts(), []);
    }

    mod user_project {
        use super::*;

        fn home_fixture() -> Fixture {
            Fixture::user()
        }

        #[test]
        fn writes_every_harness_the_expanded_install_root() {
            let f = home_fixture();
            write_catalog(&f, &["claude", "cursor"]);

            let result = f.cli(&["install"]);

            assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

            let absolute = format!("{}/{HOOK_DIR}/{SCRIPT}", to_slash(&f.install_dir));

            assert_eq!(
                f.settings(),
                json!({
                    "hooks": {
                        "PreToolUse": [{
                            "matcher": "Bash",
                            "hooks": [{ "type": "command", "command": absolute }],
                        }],
                        "Stop": [announce_entry()],
                    },
                })
            );
            assert_eq!(
                f.file_json(".cursor/hooks.json"),
                json!({
                    "version": 1,
                    "hooks": {
                        "preToolUse": [{ "command": absolute }],
                        "stop": [{ "command": "npx --yes say done" }],
                    },
                })
            );
            assert!(!f.settings_text().contains("CLAUDE_PROJECT_DIR"));
        }

        #[test]
        fn agrees_with_status_which_reads_the_scope_off_the_same_root() {
            let f = home_fixture();
            write_catalog(&f, &["claude", "cursor"]);
            assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

            assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
        }

        #[test]
        fn keeps_the_config_state_and_ignore_block_in_the_project_directory_and_none_in_home() {
            let f = home_fixture();
            write_catalog(&f, &["claude"]);

            assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

            assert!(
                f.project_dir
                    .join(STATE_DIRNAME)
                    .join(STATE_FILENAME)
                    .is_file()
            );
            assert!(f.project_dir.join("ambit.lock").is_file());
            assert!(!f.path_exists("ambit.lock"));
            assert!(f.path_exists(&format!("{HOOK_DIR}/{SCRIPT}")));

            let ignore = read_text(&f.project_dir.join(".gitignore")).unwrap();
            assert!(
                ignore.contains(&format!("\n{STATE_DIRNAME}/\n")),
                "{ignore}"
            );
            assert!(!ignore.contains(HOOK_DIR), "{ignore}");
            assert!(!f.path_exists(".gitignore"));
            assert!(!f.path_exists(".agents/.gitignore"));
        }

        #[test]
        fn cleans_the_installed_files_out_of_the_home_directory() {
            let f = home_fixture();
            write_catalog(&f, &["claude"]);
            assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

            assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);

            assert!(!f.path_exists(&format!("{HOOK_DIR}/{SCRIPT}")));
            assert!(!f.settings_text().contains(SCRIPT));
            assert!(!f.project_dir.join(STATE_DIRNAME).exists());
            assert!(f.project_dir.join("ambit.yml").is_file());
        }

        #[test]
        fn is_reached_from_any_directory_with_the_user_flag() {
            let f = home_fixture();
            write_catalog(&f, &["claude"]);

            let installed = run_cli(&["install", "--user"], &f.root, &f.env);

            assert_eq!(installed.code, ExitCode::Success, "{}", installed.stderr);
            assert!(
                f.project_dir
                    .join(STATE_DIRNAME)
                    .join(STATE_FILENAME)
                    .is_file()
            );
            assert!(f.settings_text().contains(&to_slash(&f.install_dir)));
            assert_eq!(
                run_cli(&["status", "--check", "--user"], &f.root, &f.env).code,
                ExitCode::Success
            );
        }

        #[test]
        fn refuses_the_user_flag_beside_a_project_directory() {
            let f = home_fixture();
            let project = f.project_dir.to_string_lossy().into_owned();

            let result = run_cli(
                &["status", "--user", "--project", &project],
                &f.root,
                &f.env,
            );

            assert_eq!(result.code, ExitCode::Config);
        }

        #[test]
        fn initializes_the_user_project_creating_its_directory() {
            let f = home_fixture();
            rm_rf(&f.project_dir).unwrap();

            let result = run_cli(&["init", "--user"], &f.root, &f.env);

            assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
            assert!(f.project_dir.join("ambit.yml").is_file());
            assert!(f.project_dir.join("skills/.gitkeep").is_file());
        }
    }
}
