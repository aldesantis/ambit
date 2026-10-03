//! Hooks end to end: a project's own `hooks/<name>/hook.yml` through `install`, `status`, `prune`
//! and `clean`.
//!
//! `.claude/settings.json` is what this whole capability turns on. It is not ambit's document (a
//! person's `model`, their `permissions` and hooks they wrote themselves live in it) and the tool
//! ambit replaces rewrites its entire `hooks` root on every install, destroying whatever was there.
//! So the claims below are measured in bytes rather than in behaviour, and the coexistence case is
//! asserted as whole-file text at every step of a full install, install, prune, clean cycle.
//!
//! `.cursor/hooks.json` then says the same claims hold for a harness shaped differently in every
//! respect one can be: a file of its own, its own event names, and a root key ambit seeds but does
//! not own. And `.codex/hooks.json` says they hold for the harness that differs in one respect only:
//! Claude's entries, somewhere else.
//!
//! opencode closes the set from the other end. It expresses no hooks at all, so a project that
//! configures it and declares one is warned and left installed: the case that must not be an error,
//! since one harness's limitation cannot be allowed to cost every other harness its hooks.
//!
//! No catalog but the project itself in any of that. Every definition lives in a file, so a project
//! that declares a hook of its own puts it in `hooks/` and lists itself with `source: path:.`, which
//! is both the shortest way to walk the entire path and the case worth walking, since it is what a
//! project shipping its own hook actually writes. The last block is the one exception, and it has
//! to be: it needs a hook whose script lives somewhere the project does not.

use std::path::{Path, PathBuf};

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
use crate::util::path::join;

/// The file Claude Code reads, and VS Code with it.
const SETTINGS: &str = ".claude/settings.json";

/// One hook as its own document: the directory it sits in, and the lines beyond `name`.
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

/// The hook whose event array a person has already written into.
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

/// The hook whose event array is ambit's alone, so removing it empties one.
fn notify_hook() -> Hook {
    hook(
        "notify",
        &["event: Stop", "type: command", "command: ./bin/notify"],
    )
}

/// What each of them is written as: the renderer's output, which the digest is taken over.
fn format_entry() -> JsonValue {
    json!({
        "matcher": "Write",
        "hooks": [{ "type": "command", "command": "npx prettier --write", "timeout": 30 }],
    })
}

fn notify_entry() -> JsonValue {
    json!({ "hooks": [{ "type": "command", "command": "./bin/notify" }] })
}

/// And the keys state records them under: `hooks.<Event>@<digest>`.
fn format_key() -> String {
    managed_key("hooks", &array_entry_key("PostToolUse", &format_entry()))
}

fn notify_key() -> String {
    managed_key("hooks", &array_entry_key("Stop", &notify_entry()))
}

/// `JSON.stringify(value, null, 2)` plus the trailing newline every written document carries.
fn pretty(value: &JsonValue) -> String {
    format!("{}\n", stringify_pretty(value))
}

/// A harness config as state records it.
fn config_artifact(path: &str, keys: Vec<String>) -> OwnedArtifact {
    OwnedArtifact {
        path: path.to_owned(),
        kind: ArtifactKind::HarnessConfig,
        mode: None,
        managed_keys: Some(keys),
        format: Some(DocumentFormat::Json),
        shape: Some(DocumentShape::Array),
    }
}

/// What one CLI run printed, each stream's lines joined with `\n` and no trailing newline.
struct Output {
    code: ExitCode,
    stdout: String,
    stderr: String,
}

/// One disposable root holding the project, and whatever catalog a case writes beside it.
struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    project_dir: PathBuf,
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
            project_dir,
            env,
        }
    }

    fn set_home(&mut self, home: &Path) {
        self.env
            .insert("HOME".to_owned(), home.to_string_lossy().into_owned());
    }

    /// Points a project at itself as its only catalog, and gives it `hooks` to ship.
    ///
    /// Each hook is required by the pack `core`, which the project then holds, so shipping it is
    /// what puts it in the bundle; a project declaring none holds nothing, since an entry no item
    /// matches is exit 3.
    ///
    /// `hooks/` is rebuilt from scratch on every call, because the cases that narrow a declaration
    /// have to *remove* a hook rather than leave its document beside a config that no longer
    /// selects it.
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

        // The pack these cases select, gathering whichever hooks they wrote: nothing labels itself,
        // so a grouping is a document, and this is that document.
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

    /// One of the project's files as bytes.
    fn file_text(&self, relative: &str) -> String {
        read_text(&join(&self.project_dir, relative)).unwrap()
    }

    /// The settings file as bytes.
    fn settings_text(&self) -> String {
        self.file_text(SETTINGS)
    }

    /// The settings file parsed, for the claims that are about content rather than bytes.
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
            lstat_kind(&join(&self.project_dir, relative)),
            Ok(EntryKind::Missing) | Err(_)
        )
    }

    /// Where a project path points, or `None` when it is not a symlink at all.
    fn link_target(&self, relative: &str) -> Option<String> {
        std::fs::read_link(join(&self.project_dir, relative))
            .ok()
            .map(|target| target.to_string_lossy().into_owned())
    }
}

/// One `requires` entry, taking a whole pack from `catalog`.
fn requires_entry(pack: &str, catalog: &str) -> String {
    format!("  - {{ pack: \"{catalog}/{pack}\" }}")
}

mod claude_settings {
    //! A project's own hook installed into `.claude/settings.json`.

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
        // `shape` beside `format`, because prune and clean edit this file from state alone and the
        // two JSON shapes are not the same edit.
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
        // Exit 0 rather than 5: the digest in the file is the digest state claims, so nothing
        // drifted.
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    #[test]
    fn prunes_the_entry_a_narrowed_config_no_longer_declares_leaving_the_array_behind() {
        let f = fixture();
        f.cli(&["install"]);
        f.write_profile(&[format_hook()], &["claude"]);

        assert_eq!(f.cli(&["prune"]).code, ExitCode::Success);

        // `Stop: []` survives for the reason the map driver leaves `{}`: ambit owns entries in this
        // file and not its containers, and a person may be about to add a hook of their own to that
        // array.
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

        // A project that never asked for hooks should not acquire a settings file, the same way one
        // that selects no servers acquires no `.mcp.json`.
        assert!(!f.path_exists(SETTINGS));
    }
}

mod claude_and_vscode {
    //! VS Code reads Claude's settings file natively, so the two share it.
    //!
    //! The same relationship as Claude and Cursor sharing one skills link: two harnesses naming one
    //! target is one artifact, not two that collide.

    use super::*;

    fn fixture() -> Fixture {
        let fixture = Fixture::new();
        fixture.write_profile(&[format_hook()], &["claude", "vscode"]);
        fixture
    }

    #[test]
    fn writes_the_shared_file_once_and_records_it_once() {
        let f = fixture();
        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        // One entry, not one per harness reading it.
        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PostToolUse": [format_entry()] } })
        );
        assert_eq!(
            f.state_artifacts(),
            [config_artifact(SETTINGS, vec![format_key()])]
        );
        // And one row, so `install` does not report the same write twice.
        assert_eq!(result.stdout.matches(SETTINGS).count(), 1);
    }

    #[test]
    fn leaves_vs_codes_own_config_alone_having_nothing_to_put_in_it() {
        let f = fixture();
        f.cli(&["install"]);

        assert!(!f.path_exists(".vscode/mcp.json"));
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }
}

mod handwritten_settings {
    //! The headline promise, as a byte claim.
    //!
    //! A person's `.claude/settings.json`, holding two hooks ambit knows nothing about and two
    //! sibling keys that are none of its business, through the full cycle. This is the case
    //! dotagents fails (one `[[hooks]]` entry there and the whole `hooks` root is replaced), so it
    //! is asserted as whole-file text at every step rather than as a property of one of them.

    use super::*;

    /// What the user wrote, in the form the JSON driver emits.
    ///
    /// Deliberately already canonical, so that "byte-identical" below is a claim about content
    /// rather than about whether ambit reformatted the file: a claim [`expected`] re-states in its
    /// first assertion.
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

    /// That document with `mutate` applied to its `hooks`, and nothing else touched.
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

        // The precondition the rest of this test rests on: the user's own bytes are what the driver
        // emits, so any difference below is ambit changing content and not ambit reindenting.
        assert_eq!(expected(|_| {}), HANDWRITTEN);

        // ambit's own PostToolUse hook joins the array the user already had one in; its Stop hook
        // creates one. Both appear after what was there, and the sibling keys keep their positions.
        let installed = expected(|hooks| {
            hooks["PostToolUse"]
                .as_array_mut()
                .unwrap()
                .push(format_entry());
            hooks.insert("Stop".to_owned(), json!([notify_entry()]));
        });

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        assert_eq!(f.settings_text(), installed);

        // A second install appends nothing: the digests are already there.
        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        assert_eq!(f.settings_text(), installed);

        // A prune with nothing stale writes nothing at all.
        assert_eq!(f.cli(&["prune"]).code, ExitCode::Success);
        assert_eq!(f.settings_text(), installed);

        // And `clean` takes back exactly ambit's two entries. The emptied `Stop` array stays, for
        // the reason the map driver leaves `{}` behind; the user's own array keeps its foreign
        // entry.
        assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);
        assert_eq!(
            f.settings_text(),
            expected(|hooks| {
                hooks.insert("Stop".to_owned(), json!([]));
            })
        );

        // Which is to say: the file still holds exactly what the user wrote.
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

        // The two hand-written entries have digests ambit never plans, so they are not in state, so
        // nothing can ever prune them. The promise falls out of the identity scheme rather than a
        // rule.
        let artifacts = f.state_artifacts();

        assert_eq!(
            artifacts[0].managed_keys,
            Some(vec![format_key(), notify_key()])
        );
    }
}

mod identical_handwritten_entry {
    //! The one pre-existing entry that *is* a conflict: a person who hand-wrote the very hook ambit
    //! is about to install. Refusing it and offering `--adopt` is what `.mcp.json` does for a
    //! colliding server name, and the digest is what makes the same question askable of an array.

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
        // Nothing at all was written: the check runs over the whole plan before any adapter applies.
        assert!(!f.path_exists(STATE_DIRNAME));
    }

    #[test]
    fn takes_it_over_under_adopt_without_writing_a_second_copy_of_it() {
        let f = fixture();
        let result = f.cli(&["install", "--adopt"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        // The digest is already present, so the merge appends nothing: the array holds one entry.
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
    //! A digest that stops matching what state recorded, from each of the two sides it can stop
    //! from.
    //!
    //! The digest is the identity, so "this entry changed" is not a thing the driver can say: a
    //! changed entry is a key that is absent and a different key that is present. That is what
    //! makes `status` the load-bearing half of the story. Without a row saying so, an install would
    //! put ambit's entry back beside the one that no longer matches and the file would quietly grow
    //! a second hook on the event.
    //!
    //! Both sides are here because the residue differs. Edit the *file* and ambit cannot tell its
    //! own former entry from a hook the person wrote, so it restores its own and leaves theirs,
    //! which is the ownership rule rather than an exception to it. Edit the *declaration* and the
    //! stale digest is one state claims, so the next install takes it out and writes the current
    //! one: one entry, not two.

    use super::*;

    /// The same hook with its timeout raised: what a person editing `hook.yml` would leave.
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

    /// ambit's own entry, edited in place in the file rather than in the declaration.
    fn edited_entry() -> JsonValue {
        json!({
            "matcher": "Write",
            "hooks": [{ "type": "command", "command": "npx prettier --write", "timeout": 60 }],
        })
    }

    /// Rewrites the installed entry's timeout, the way someone tweaking the file by hand would.
    fn edit_installed_entry(f: &Fixture) {
        let text = f.settings_text();

        assert!(text.contains("\"timeout\": 30"));
        write_text(
            &join(&f.project_dir, SETTINGS),
            &text.replace("\"timeout\": 30", "\"timeout\": 60"),
        )
        .unwrap();
    }

    /// Every artifact row `status --json` reported.
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
        // Exit 5, so a CI job finds out before the install that heals it.
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Drift);
    }

    #[test]
    fn puts_its_own_entry_back_on_the_next_install_and_leaves_the_edit_as_the_persons() {
        let f = fixture();
        edit_installed_entry(&f);

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        // Two entries, and neither is a duplicate of the other: the edited one has a digest ambit
        // never plans, which is indistinguishable from a hook the person wrote themselves, so it
        // stays, exactly as the foreign entries above do. Ambit's own is written once, at the end of
        // the array.
        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PostToolUse": [edited_entry(), format_entry()] } })
        );
        assert_eq!(
            f.state_artifacts()[0].managed_keys,
            Some(vec![format_key()])
        );

        // And that is a settled state rather than a file that grows: the row is `ok` again, and a
        // further install appends nothing.
        let healed = f.settings_text();

        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
        assert_eq!(f.settings_text(), healed);
    }

    #[test]
    fn reports_the_digest_a_changed_declaration_now_wants_as_missing() {
        let f = fixture();
        f.write_profile(&[retimed_hook()], &["claude"]);

        // The new digest, not the old one: the row is a function of the bundle, so it names the
        // entry install would write rather than the one that happens to be in the file.
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

        // One entry. The old digest is one state claimed and the plan no longer writes, so pruning
        // takes it out of the array: the same rule that retires a withdrawn hook, applied to a
        // redeclared one.
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
    //! Cursor, which is where the neutral vocabulary earns itself.
    //!
    //! A second harness that differs in every respect one can: its own file, its own spelling of
    //! every event, an entry that nests nothing and has nowhere to put a `matcher`, and a `version`
    //! beside the hooks that ambit seeds and a person owns. None of which the install path knows: it
    //! is the same code that wrote Claude's file above, reading a different profile.

    use super::*;

    const HOOKS_JSON: &str = ".cursor/hooks.json";

    /// A hook with nothing optional on it, so an event is the only thing varying below.
    fn watch_entry() -> JsonValue {
        json!({ "command": "./bin/watch" })
    }

    /// The `watch` hook on `event`, as a fresh profile.
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

    /// Every event ambit knows, and what Cursor calls it.
    ///
    /// Written out rather than read off the profile: the map is the claim, so a test that imported
    /// it would agree with any spelling the profile happened to hold.
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

            // `version` first, because ambit created the file and Cursor's own documentation writes
            // it there.
            let mut hooks = crate::util::json::JsonObject::new();
            hooks.insert(spelling.to_owned(), json!([watch_entry()]));
            assert_eq!(
                f.file_text(HOOKS_JSON),
                pretty(&json!({ "version": 1, "hooks": hooks })),
                "{event}"
            );
            // And the managed key names the array the entry actually sits in. If it named the
            // PascalCase event instead, `section_keys` would not recognize what ambit just wrote
            // and the next install would append the hook a second time.
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

        // Written through, `matcher` would be a key Cursor ignores, so the hook would silently run
        // on every tool while the file claimed otherwise. Dropped, it runs unfiltered and says so.
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
        // A `version` a person raised themselves, and a hook of their own on the event ambit writes
        // to. dotagents replaces this file's `hooks` root and forces `version` back to 1; ambit owns
        // one entry.
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

        // And `clean` gives back exactly what they wrote, `version: 2` included.
        assert_eq!(f.cli(&["clean"]).code, ExitCode::Success);
        assert_eq!(f.file_text(HOOKS_JSON), handwritten);
    }
}

mod codex_hooks {
    //! Codex: Claude's entries, in a file of its own.
    //!
    //! The pairing that proves the layout and the renderer are separable: Codex shares Claude's hook
    //! rendering outright and shares nothing else, so a hook has the same digest here as in
    //! `.claude/settings.json` and lands somewhere else entirely.
    //!
    //! `.codex/hooks.json` and not `[hooks]` in `.codex/config.toml`, which Codex also reads: a TOML
    //! `hooks` table is an array-of-tables, which the TOML driver refuses. So the test also pins
    //! that ambit leaves `config.toml` alone: a project on Codex with hooks and no servers acquires
    //! no TOML at all.

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

        // No `version` and no other root key: unlike Cursor's file this one holds hooks and nothing
        // else, so ambit seeds nothing beside them.
        assert_eq!(
            f.file_text(CODEX_HOOKS),
            pretty(&json!({
                "hooks": { "PostToolUse": [format_entry()], "Stop": [notify_entry()] },
            }))
        );
        // The same digests Claude's file would carry, because the entries are byte-identical.
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
        // The emptied array stays, for the reason the map driver leaves `{}`; the foreign entry
        // never moved.
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

        // Two artifacts rather than one: the entries are identical, so it is the *file* that makes
        // them two writes. `plan_for` collapses two harnesses onto one target and never two targets
        // onto one.
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

mod opencode_skip {
    //! opencode, which expresses no hooks at all.
    //!
    //! A harness in `harnesses` that cannot take a hook is not an error. Failing would let one
    //! harness veto every other harness's hooks, and dropping the hook in silence would leave a
    //! project believing it installed something. So the run succeeds, writes what it can, and says
    //! what it could not.

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
        // No file of opencode's, and no settings file either: the hook reached no harness, so
        // nothing at all was written for it.
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

        // Claude's file is written in full. The skip is opencode's alone, which is the reason it is
        // a warning: one harness's limitation must not cost the others their hooks.
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
        // Still on stderr, so stdout stays a document a script can parse.
        assert!(result.stderr.contains("no declarative hook mechanism"));
    }
}

mod claude_and_cursor {
    //! Claude and Cursor together: two harnesses, two files, two renderings.
    //!
    //! The counterpart of the Claude/VS Code case above. There the two shared a file because they
    //! render one entry; here they render different entries into different files, so `plan_for`
    //! collapses nothing.

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
    //! A hook that ships its own script: the thing dotagents cannot do at all.
    //!
    //! The only case here that needs a catalog: shipping bytes needs a directory to ship them from,
    //! and `ambit.yml` has none. So the hook's directory is materialized under
    //! `.agents/hooks/<name>/` exactly as a skill's is under `.agents/skills/<name>/`, which is what
    //! makes a hook a dependency a project resolves rather than a script every consumer commits for
    //! themselves.
    //!
    //! `hook-dir` is an artifact kind dispatched on in five places where the wrong branch compiles
    //! clean. Each of those has a test below saying so in its own name, because a passing build says
    //! nothing about any of them: a kind routed into the config arm, or left out of an allow-list,
    //! is a silent wrong answer rather than a failure.

    use super::*;
    use crate::project::doctor::{DoctorOptions, diagnose_project};
    use crate::project::gitignore::{BLOCK_BEGIN, BLOCK_END, SHARED_GITIGNORE_FILE};
    use crate::util::cmp::js_cmp;

    const SCRIPT_HOOK: &str = "block-rm";
    const HOOK_DIR: &str = ".agents/hooks/block-rm";
    const SCRIPT: &str = "hook.sh";
    const SCRIPT_BODY: &str = "#!/bin/sh\nexit 0\n";

    /// Where each harness family is pointed at the materialized script.
    ///
    /// The declaration is `command: hook.sh`, which names a file relative to the hook's directory
    /// *in the catalog*, so what reaches a config file has to name the installed copy instead,
    /// spelled the way that harness resolves a path. Claude and VS Code get Claude's documented
    /// `${CLAUDE_PROJECT_DIR}`; Cursor and Codex interpolate nothing, so they get the path
    /// project-relative.
    fn claude_command() -> String {
        format!("${{CLAUDE_PROJECT_DIR}}/{HOOK_DIR}/{SCRIPT}")
    }

    fn relative_command() -> String {
        format!("{HOOK_DIR}/{SCRIPT}")
    }

    /// What the hook renders as in Claude's file.
    ///
    /// Kept in one place because the digest, and so every managed key state records, is taken over
    /// exactly these bytes: one constant changes when the rendering does.
    fn script_entry() -> JsonValue {
        json!({ "matcher": "Bash", "hooks": [{ "type": "command", "command": claude_command() }] })
    }

    /// The hook in the same catalog whose `command` is a command line, so it ships nothing.
    fn announce_entry() -> JsonValue {
        json!({ "hooks": [{ "type": "command", "command": "npx --yes say done" }] })
    }

    /// Both entries' keys, in the order state records them.
    fn hook_keys() -> Vec<String> {
        let mut keys = vec![
            managed_key("hooks", &array_entry_key("PreToolUse", &script_entry())),
            managed_key("hooks", &array_entry_key("Stop", &announce_entry())),
        ];
        keys.sort_by(|a, b| js_cmp(a, b));
        keys
    }

    /// A catalog beside the project holding one hook that ships a script and one that does not.
    ///
    /// Two hooks rather than one, because "only a script-shipping hook plans a directory" is half
    /// the claim: a command-line hook in the same bundle has to plan a config entry and nothing
    /// else.
    fn write_catalog(f: &Fixture, harnesses: &[&str]) {
        let catalog_dir = f.root.join("catalog");
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

        // Executable in the catalog, which is the only reason `--copy` preserving it is a claim.
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

        // The script is reachable at the shared location, and the settings entry is written beside
        // it.
        assert_eq!(f.file_text(&format!("{HOOK_DIR}/{SCRIPT}")), SCRIPT_BODY);
        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PreToolUse": [script_entry()], "Stop": [announce_entry()] } })
        );

        // `hook-dir`, not `skill-dir`: state is what prune, `clean` and `status` act from, and a
        // hook's directory reported as a skill's would be a lie in each of them. The catalog is a
        // `path:` source, so the mode is `link`: the same rule a skill follows.
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
                },
                config_artifact(SETTINGS, hook_keys()),
            ]
        );
    }

    #[test]
    fn plans_no_directory_for_the_command_line_hook_beside_it() {
        let f = fixture();
        f.cli(&["install"]);

        // `npx --yes say done` names no file the catalog holds, so it is a command line: config
        // entry, no bytes. Only the script-shipping hook has anything to materialize.
        assert!(!f.path_exists(".agents/hooks/announce"));
        assert_eq!(
            f.state_artifacts()
                .iter()
                .filter(|artifact| artifact.kind == ArtifactKind::HookDir)
                .count(),
            1
        );
    }

    /// The command each harness is actually given, from an install rather than from a renderer.
    ///
    /// The one string the whole capability turns on: `command: hook.sh` is relative to a directory
    /// in the catalog, which is a place no harness has heard of, so an unrewritten command installs
    /// a hook that silently never fires. Four harnesses, one materialized script, two spellings of
    /// the way to it, and exact strings, because a placeholder a harness does not interpolate is not
    /// a near miss.
    #[test]
    fn writes_the_materialized_path_the_way_each_harness_resolves_one() {
        let f = fixture();
        write_catalog(&f, &["claude", "codex", "cursor", "vscode"]);

        let result = f.cli(&["install"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        // One script, however many harnesses read it.
        assert_eq!(f.file_text(&format!("{HOOK_DIR}/{SCRIPT}")), SCRIPT_BODY);

        // Claude, and VS Code out of the same file: Claude's own documented placeholder, which holds
        // the project root, so the script is found whatever a session's cwd is.
        assert_eq!(
            f.settings(),
            json!({ "hooks": { "PreToolUse": [script_entry()], "Stop": [announce_entry()] } })
        );
        assert!(f.settings_text().contains(&claude_command()));

        // Codex: Claude's entry shape, and not Claude's path: it interpolates nothing. Notably *not*
        // `$(git rev-parse --show-toplevel)/…`, which its own docs suggest and ambit will not write.
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

        // Cursor: its own flat entry, its own camelCased events, and the same project-relative path.
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

        // And the hook that ships nothing is written verbatim into all three: prefixing a command
        // line with a directory would break it, and there are no bytes there to point at.
        for file in [SETTINGS, ".codex/hooks.json", ".cursor/hooks.json"] {
            assert!(f.file_text(file).contains("npx --yes say done"), "{file}");
            assert!(!f.file_text(file).contains("hooks/announce"), "{file}");
        }
    }

    /// `planned_paths` (`project/prune.rs`): the allow-list that decides which owned paths this run
    /// still writes.
    ///
    /// A `hook-dir` missing from it makes the directory look stale on every install: pruning runs
    /// after `apply`, so ambit would delete the script it just wrote, recreate it next run, and
    /// leave the settings entry pointing at nothing in between.
    #[test]
    fn keeps_the_directory_on_a_second_install_rather_than_deleting_and_rewriting_it() {
        let f = fixture();
        f.cli(&["install"]);
        let written = f.settings_text();

        // What a second run would remove, answered from state and the plan alone: nothing at all.
        let preview = f.cli(&["install", "--dry-run", "--json"]);

        assert_eq!(preview.code, ExitCode::Success, "{}", preview.stderr);
        assert_eq!(parse(&preview.stdout).unwrap()["pruned"], json!([]));

        assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

        // And pruning runs *after* `apply`, so a directory wrongly judged stale is deleted having
        // just been written, leaving the settings entry naming a script the project does not hold.
        assert_eq!(f.file_text(&format!("{HOOK_DIR}/{SCRIPT}")), SCRIPT_BODY);
        assert_eq!(f.settings_text(), written);
        assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
    }

    /// `compare_artifacts` (`project/status.rs`): which verdict function a kind is compared by.
    ///
    /// Without a branch of its own a `hook-dir` falls past both path branches into the config arm,
    /// where `config_verdict` reads the directory as a document.
    #[test]
    fn reports_the_directory_as_a_hook_dir_compared_as_a_directory() {
        let f = fixture();
        f.cli(&["install"]);

        let artifacts = status_artifacts(&f);

        assert!(
            artifacts.contains(&json!({ "kind": "hook-dir", "path": HOOK_DIR, "state": "ok" }))
        );
    }

    /// `compare_artifacts` again, from the other side: a verdict has to be able to say `modified`.
    ///
    /// A row that reads `ok` whatever the directory holds would satisfy the test above while
    /// reporting nothing, so the comparison is also asked about a script someone edited.
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

    /// `apply` (`harness/profile.rs`): which writer a kind is handed to.
    ///
    /// The fallback there is the harness-config writer, so a `hook-dir` without a branch of its own
    /// would have a `hooks` section merged into it as though the directory were a JSON file.
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

        // A real directory now, holding the bytes rather than pointing at them.
        assert_eq!(f.link_target(HOOK_DIR), None);
        assert_eq!(f.file_text(&format!("{HOOK_DIR}/{SCRIPT}")), SCRIPT_BODY);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_the_script_executable_through_a_copy_install() {
        use std::os::unix::fs::PermissionsExt;

        let f = fixture();
        assert_eq!(f.cli(&["install", "--copy"]).code, ExitCode::Success);

        // A copy preserves mode, and a hook the harness cannot execute is a hook that does not run,
        // so the bit is part of what the catalog ships rather than something the project has to
        // restore.
        let mode = std::fs::symlink_metadata(join(&f.project_dir, &format!("{HOOK_DIR}/{SCRIPT}")))
            .unwrap()
            .permissions()
            .mode();

        assert_eq!(mode & 0o111, 0o111);
    }

    /// `gitignore_blocks` (`project/gitignore.rs`): which kinds are listed at all.
    ///
    /// The loop skips anything it does not recognize, so a `hook-dir` left out means every copied
    /// script shows up as untracked in `git status`. The path is under `.agents/`, so it lands in the
    /// volatile nested block for free once the kind is admitted.
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

        // Anchored and without a trailing slash, exactly as a skill's pattern is: the default
        // install is a symlink, which git does not match a `dir/` pattern against.
        assert_eq!(lines[start + 1..end], [format!("/hooks/{SCRIPT_HOOK}")]);
    }

    /// `mode_findings` (`project/doctor.rs`): which kinds the mode check counts.
    ///
    /// `status` is deliberately silent about materialization mode, so this is the only command that
    /// can report it. Counting only `skill-dir` means a hook's script installed with `--copy` out of
    /// a working copy is never mentioned anywhere.
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

        // A warning, not a failure: `--copy` is a per-run flag, and both modes put the same bytes in
        // front of the harness. What is worth saying is that the next plain install will swap it
        // back.
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
        // The bytes it was copied from are the catalog's, and `clean` is about the project.
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

        // The other half of `planned_paths`: a path the plan no longer writes *is* stale, and
        // pruning it is what stops a withdrawn hook's script from sitting in the project forever.
        assert!(!f.path_exists(HOOK_DIR));
        assert_eq!(f.state_artifacts(), []);
    }

    mod home_as_project_root {
        //! The same install with the home directory as its root: how a person gets one set of hooks
        //! in every project at once.
        //!
        //! `~/.claude/settings.json` is not a project's settings file: Claude Code reads it as the
        //! user's own, and applies it to every project on the machine. Cursor and Codex read their
        //! files under `~` the same way. So the paths the cases above assert are all wrong here, and
        //! wrong twice over: a hook naming `${CLAUDE_PROJECT_DIR}/.agents/hooks/…` or
        //! `.agents/hooks/…` resolves inside whichever project happens to be open, which silently
        //! finds nothing in most of them and finds *that project's* script in one that ships the
        //! same path.
        //!
        //! Nothing declares the scope. `install_scope` (`project/install.rs`) reads it off the root,
        //! so a home install cannot forget to ask for the safe spelling. `HOME` is set in the `Env`
        //! each run is given, never in the process.

        use super::*;

        fn home_fixture() -> Fixture {
            let mut f = Fixture::new();
            let home = f.project_dir.clone();
            f.set_home(&home);
            f
        }

        #[test]
        fn writes_every_harness_the_expanded_install_root() {
            let f = home_fixture();
            write_catalog(&f, &["claude", "cursor"]);

            let result = f.cli(&["install"]);

            assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

            // The one path that reaches the script from every project, since it depends on none of
            // them.
            let absolute = format!("{}/{HOOK_DIR}/{SCRIPT}", f.project_dir.display());

            assert_eq!(
                f.settings(),
                json!({
                    "hooks": {
                        "PreToolUse": [{
                            "matcher": "Bash",
                            "hooks": [{ "type": "command", "command": absolute }],
                        }],
                        // The command-line hook beside it, which has no script to address and so is
                        // untouched.
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
        fn replaces_a_project_relative_entry_an_earlier_install_left_rather_than_leaving_both() {
            let mut f = home_fixture();
            write_catalog(&f, &["claude"]);
            // The same root installed as a project, which is what a home install used to write:
            // `root` is the parent here, so nothing about this run is user-level.
            let root = f.root.clone();
            f.set_home(&root);
            assert_eq!(f.cli(&["install"]).code, ExitCode::Success);
            assert!(f.settings_text().contains("CLAUDE_PROJECT_DIR"));

            let home = f.project_dir.clone();
            f.set_home(&home);
            let result = f.cli(&["install"]);

            assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

            // One entry, not two. The old key is state's, so pruning takes it out: leaving it would
            // keep a hook pointed into whatever project is open, which is the entry that had to go.
            assert_eq!(
                f.settings(),
                json!({
                    "hooks": {
                        "PreToolUse": [{
                            "matcher": "Bash",
                            "hooks": [{
                                "type": "command",
                                "command": format!("{}/{HOOK_DIR}/{SCRIPT}", f.project_dir.display()),
                            }],
                        }],
                        "Stop": [announce_entry()],
                    },
                })
            );
        }

        #[test]
        fn agrees_with_status_which_reads_the_scope_off_the_same_root() {
            let f = home_fixture();
            write_catalog(&f, &["claude", "cursor"]);
            assert_eq!(f.cli(&["install"]).code, ExitCode::Success);

            // Both commands plan through the adapters, so a scope only install knew about would make
            // every status report drift on a project nobody touched, and every install rewrite the
            // entry.
            assert_eq!(f.cli(&["status", "--check"]).code, ExitCode::Success);
        }
    }
}
