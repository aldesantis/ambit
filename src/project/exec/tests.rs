//! The `exec` digest recipe, and the gate an install runs on it.
//!
//! The digests are checked field by field on hand-built items. The gate is checked twice over:
//! purely, through [`review_exec`] on locks [`build_lock`] makes (which is how a commit and a
//! script's tree digest reach it without git), and end to end through `ambit install` on the
//! fixture catalog, which is a `path:` source and so has to opt in with `trust: review`. The git
//! side, where `review` is the default, is covered in `project/update/tests.rs`.

use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;
use crate::errors::ExitCode;
use crate::model::catalog::{
    Catalog, CatalogParseOptions, merge_catalogs, parse_catalog_directory,
};
use crate::model::config::load_project_config;
use crate::model::hook_entity::HookEvent;
use crate::model::lock_file::read_locked_items;
use crate::model::mcp_entity::{HttpTransport, StdioTransport};
use crate::project::install::fixture::*;
use crate::project::lock::{
    ItemDigests, LOCK_FILENAME, build_lock, serialize_lock, write_lock_text,
};
use crate::resolution::resolve::resolve_bundle;

// The recipe

fn hook(r#type: HookType, command: &str) -> MergedHook {
    MergedHook {
        name: "guard".to_owned(),
        description: None,
        event: HookEvent::PreToolUse,
        matcher: Some("Bash".to_owned()),
        r#type,
        command: command.to_owned(),
        timeout: None,
        expects: Vec::new(),
        catalog: CATALOG_NAME.to_owned(),
        path: "hooks/guard".to_owned(),
        commit: None,
        catalog_root: PathBuf::new(),
    }
}

fn stdio(command: &str, args: &[&str], env: &[(&str, &str)]) -> McpTransport {
    McpTransport::Stdio(StdioTransport {
        command: command.to_owned(),
        args: args.iter().map(|&arg| arg.to_owned()).collect(),
        env: env
            .iter()
            .map(|&(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
    })
}

fn http(url: &str, bearer: Option<&str>, headers: &[(&str, &str)]) -> McpTransport {
    McpTransport::Http(HttpTransport {
        url: url.to_owned(),
        bearer_token_env_var: bearer.map(str::to_owned),
        headers: headers
            .iter()
            .map(|&(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
    })
}

#[test]
fn a_hook_digest_is_stable_and_moves_with_every_field_that_decides_what_runs() {
    let base = hook(HookType::Command, "echo hi");
    let digest = hook_exec(&base, None);

    assert_eq!(hook_exec(&base.clone(), None), digest);
    assert!(digest.starts_with("sha256-"));

    let mut event = base.clone();
    event.event = HookEvent::PostToolUse;

    let mut matcher = base.clone();
    matcher.matcher = None;

    let mut empty_matcher = base.clone();
    empty_matcher.matcher = Some(String::new());

    let mut command = base.clone();
    command.command = "echo bye".to_owned();

    let mut r#type = base.clone();
    r#type.r#type = HookType::Script;

    for (field, changed) in [
        ("event", &event),
        ("matcher", &matcher),
        ("type", &r#type),
        ("command", &command),
    ] {
        assert_ne!(hook_exec(changed, None), digest, "{field}");
    }

    assert_ne!(hook_exec(&matcher, None), hook_exec(&empty_matcher, None));

    // Nothing else changes what runs.
    let mut cosmetic = base.clone();
    cosmetic.timeout = Some(30);
    cosmetic.description = Some("says hi".to_owned());
    cosmetic.catalog = "elsewhere".to_owned();

    assert_eq!(hook_exec(&cosmetic, None), digest);
}

#[test]
fn a_script_hook_digest_covers_the_scripts_tree_and_a_command_hook_ignores_one() {
    let script = hook(HookType::Script, "guard.sh");
    let one = hook_exec(&script, Some("sha256-one"));

    assert_ne!(hook_exec(&script, Some("sha256-two")), one);
    assert_ne!(hook_exec(&script, None), one);

    let command = hook(HookType::Command, "guard.sh");

    assert_eq!(
        hook_exec(&command, Some("sha256-one")),
        hook_exec(&command, None)
    );
}

#[test]
fn a_stdio_digest_moves_with_the_command_each_argument_and_the_environment() {
    let digest = mcp_exec(&stdio("npx", &["-y", "pkg"], &[("A", "1")]));

    for (field, changed) in [
        ("command", stdio("node", &["-y", "pkg"], &[("A", "1")])),
        ("an argument", stdio("npx", &["-y", "other"], &[("A", "1")])),
        (
            "argument order",
            stdio("npx", &["pkg", "-y"], &[("A", "1")]),
        ),
        (
            "an extra argument",
            stdio("npx", &["-y", "pkg", "x"], &[("A", "1")]),
        ),
        ("an env name", stdio("npx", &["-y", "pkg"], &[("B", "1")])),
        ("an env value", stdio("npx", &["-y", "pkg"], &[("A", "2")])),
        ("no env", stdio("npx", &["-y", "pkg"], &[])),
    ] {
        assert_ne!(mcp_exec(&changed), digest, "{field}");
    }

    // An argument cannot pass for an env entry, nor two arguments for one.
    assert_ne!(
        mcp_exec(&stdio("npx", &["A", "1"], &[])),
        mcp_exec(&stdio("npx", &[], &[("A", "1")]))
    );
    assert_ne!(
        mcp_exec(&stdio("npx", &["a b"], &[])),
        mcp_exec(&stdio("npx", &["a", "b"], &[]))
    );

    // The order a catalog wrote its variables in runs the same process.
    assert_eq!(
        mcp_exec(&stdio("npx", &[], &[("A", "1"), ("B", "2")])),
        mcp_exec(&stdio("npx", &[], &[("B", "2"), ("A", "1")]))
    );
}

#[test]
fn an_http_digest_moves_with_the_url_the_token_variable_and_each_header() {
    let digest = mcp_exec(&http("https://a", Some("TOKEN"), &[("X-Key", "${KEY}")]));

    for (field, changed) in [
        (
            "url",
            http("https://b", Some("TOKEN"), &[("X-Key", "${KEY}")]),
        ),
        (
            "bearer",
            http("https://a", Some("OTHER"), &[("X-Key", "${KEY}")]),
        ),
        ("no bearer", http("https://a", None, &[("X-Key", "${KEY}")])),
        (
            "header name",
            http("https://a", Some("TOKEN"), &[("X-Other", "${KEY}")]),
        ),
        (
            "header value",
            http("https://a", Some("TOKEN"), &[("X-Key", "${OTHER}")]),
        ),
    ] {
        assert_ne!(mcp_exec(&changed), digest, "{field}");
    }

    assert_ne!(
        mcp_exec(&http("npx", None, &[])),
        mcp_exec(&stdio("npx", &[], &[]))
    );
}

// The gate, through `review_exec`

/// The fixture catalog parsed at `commit`.
fn catalog_at(project: &Project, commit: &str) -> Catalog {
    parse_catalog_directory(
        CATALOG_NAME,
        "path:../catalog",
        &project.catalog,
        Some(commit),
        &mut CatalogParseOptions::default(),
    )
    .expect("a parsable catalog")
}

/// The lock the default profile resolves to with the catalog at `commit` and the script hook's
/// tree hashing to `tree`, and the bundle it was built from.
fn locked(project: &Project, commit: &str, tree: &str) -> (Lock, Bundle) {
    let catalogs = [catalog_at(project, commit)];
    let config = load_project_config(&project.dir).expect("a valid config");
    let bundle = resolve_bundle(&config, &merge_catalogs(&catalogs)).expect("a resolvable profile");
    let digests = ItemDigests {
        hooks: IndexMap::from([("guard-secrets".to_owned(), tree.to_owned())]),
        ..ItemDigests::default()
    };
    let lock = build_lock(&catalogs, &bundle, &digests).expect("a lock");

    (lock, bundle)
}

/// What a project holding `text` as its lock reads back.
fn read_back(text: &str) -> LockedItems {
    let project = Project::new();

    write_lock_text(&project.dir, text).unwrap();
    read_locked_items(&project.dir).unwrap().expect("a lock")
}

fn reviewed() -> IndexMap<String, Trust> {
    IndexMap::from([(CATALOG_NAME.to_owned(), Trust::Review)])
}

/// Each change as `(name, status)`, for a compact comparison.
fn summary(changes: &[ExecChange]) -> Vec<(&str, ExecStatus)> {
    changes
        .iter()
        .map(|change| (change.name.as_str(), change.status))
        .collect()
}

fn profiled() -> Project {
    let project = Project::new();

    project.write_profile(DEFAULT_PACKS, None, &[]);
    project
}

#[test]
fn passes_a_lock_that_already_holds_every_digest() {
    let project = profiled();
    let (lock, bundle) = locked(&project, "abc1234", "sha256-one");
    let previous = read_back(&serialize_lock(&lock));

    assert_eq!(
        review_exec(Some(&previous), &lock, &bundle, &reviewed()),
        ExecReview::default()
    );
}

#[test]
fn gates_a_script_whose_bytes_changed_at_a_new_commit_and_nothing_else() {
    let project = profiled();
    let (before, _) = locked(&project, "abc1234", "sha256-one");
    let (after, bundle) = locked(&project, "def5678", "sha256-two");
    let review = review_exec(
        Some(&read_back(&serialize_lock(&before))),
        &after,
        &bundle,
        &reviewed(),
    );

    assert_eq!(
        summary(&review.gated),
        [("guard-secrets", ExecStatus::Changed)]
    );
    assert_eq!(review.endpoints, []);
}

#[test]
fn gates_every_hook_and_warns_of_every_endpoint_with_no_lock_to_compare_against() {
    let project = profiled();
    let (lock, bundle) = locked(&project, "abc1234", "sha256-one");
    let review = review_exec(None, &lock, &bundle, &reviewed());

    assert_eq!(
        summary(&review.gated),
        [
            ("guard-secrets", ExecStatus::New),
            ("session-notes", ExecStatus::New)
        ]
    );
    assert_eq!(summary(&review.endpoints), [("linter", ExecStatus::New)]);
    assert_eq!(review.gated[0].note(), "new, from company@abc1234");
    assert_eq!(review.gated[0].when, "PreToolUse Bash");
    assert_eq!(review.gated[0].runs, "hooks/guard-secrets/guard.sh");
    assert_eq!(review.endpoints[0].runs, "https://mcp.invalid/fixture");
}

#[test]
fn reviews_nothing_from_a_catalog_of_full_trust() {
    let project = profiled();
    let (lock, bundle) = locked(&project, "abc1234", "sha256-one");
    let full = IndexMap::from([(CATALOG_NAME.to_owned(), Trust::Full)]);

    assert_eq!(
        review_exec(None, &lock, &bundle, &full),
        ExecReview::default()
    );
}

/// `text` with every `exec` line removed: a lock as an ambit from before `exec` wrote it.
fn without_exec(text: &str) -> String {
    text.lines()
        .filter(|line| !line.trim_start().starts_with("exec:"))
        .flat_map(|line| [line, "\n"])
        .collect()
}

#[test]
fn accepts_an_entry_without_a_digest_only_at_the_commit_it_was_recorded_at() {
    let project = profiled();
    let (before, bundle) = locked(&project, "abc1234", "sha256-one");
    let previous = read_back(&without_exec(&serialize_lock(&before)));

    assert_eq!(previous.hooks["guard-secrets"].exec, None);
    assert_eq!(
        review_exec(Some(&previous), &before, &bundle, &reviewed()),
        ExecReview::default()
    );

    // Same definitions, moved commit: nothing proves the entry is what ran before.
    let (moved, bundle) = locked(&project, "def5678", "sha256-one");
    let review = review_exec(Some(&previous), &moved, &bundle, &reviewed());

    assert_eq!(
        summary(&review.gated),
        [
            ("guard-secrets", ExecStatus::Changed),
            ("session-notes", ExecStatus::Changed)
        ]
    );
    assert_eq!(
        summary(&review.endpoints),
        [("linter", ExecStatus::Changed)]
    );
    assert_eq!(review.endpoints[0].note(), "endpoint changed");
}

#[test]
fn refuses_with_one_aligned_entry_per_change() {
    let changes = [
        ExecChange {
            kind: ItemKind::Hook,
            name: "guard-secrets".to_owned(),
            catalog: "company".to_owned(),
            commit: Some("f9e1a04c0ffee".to_owned()),
            when: "PreToolUse Bash".to_owned(),
            runs: "hooks/guard-secrets/guard.sh".to_owned(),
            status: ExecStatus::New,
        },
        ExecChange {
            kind: ItemKind::Mcp,
            name: "linear".to_owned(),
            catalog: "community".to_owned(),
            commit: None,
            when: "stdio".to_owned(),
            runs: "npx -y @acme/linear-mcp".to_owned(),
            status: ExecStatus::Changed,
        },
    ];
    let error = refuse_unaccepted(&changes).unwrap_err();

    assert_eq!(error.code, ExitCode::Drift);
    assert_eq!(
        error.format(),
        [
            "error: install would add execution that was not in the lock",
            "       hook guard-secrets  PreToolUse Bash",
            "         hooks/guard-secrets/guard.sh   (new, from company@f9e1a04)",
            "       mcp linear  stdio",
            "         npx -y @acme/linear-mcp        (command changed)",
            "       review the change, then re-run with `--accept-exec`",
        ]
        .join("\n")
    );
    refuse_unaccepted(&[]).unwrap();
}

#[test]
fn shows_a_stdio_server_as_a_command_line_led_by_its_environment() {
    let mcp = MergedMcp {
        name: "linear".to_owned(),
        transport: stdio("npx", &["-y", "a b", ""], &[("Z", "${Z}"), ("A", "1")]),
        expects: Vec::new(),
        catalog: CATALOG_NAME.to_owned(),
        file: "mcps/linear.yml".to_owned(),
    };

    assert_eq!(mcp_runs(&mcp), "A=1 Z=${Z} npx -y \"a b\" \"\"");
}

#[test]
fn shows_a_script_hook_by_its_path_in_the_catalog_with_its_arguments() {
    assert_eq!(
        hook_runs(&hook(HookType::Script, "./guard.sh --strict")),
        "hooks/guard/guard.sh --strict"
    );
    assert_eq!(hook_runs(&hook(HookType::Command, " echo hi ")), "echo hi");
}

// The gate, through `ambit install`

/// The default profile plus the skill that pulls in the fixture's stdio server and its third hook,
/// from a catalog written with `trust`, or none when `None`.
fn configured(trust: Option<&str>) -> Project {
    let project = Project::new();
    let trust_line = trust.map_or_else(String::new, |trust| format!("    trust: {trust}\n"));
    let mut requires: Vec<String> = DEFAULT_PACKS
        .iter()
        .map(|pack| requires_entry(pack))
        .collect();

    requires.push(format!(
        "  - {{ skill: \"{CATALOG_NAME}/{PROJECT_SKILL}\" }}"
    ));
    project.write(
        "ambit.yml",
        &format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\n{trust_line}requires:\n{}\n",
            requires.join("\n")
        ),
    );
    project
}

const FIRST_INSTALL_REFUSAL: &str = "\
error: install would add execution that was not in the lock
       hook acme-standup  SessionEnd
         echo \"acme session ended\"       (new, from company)
       hook guard-secrets  PreToolUse Bash
         hooks/guard-secrets/guard.sh    (new, from company)
       hook session-notes  SessionStart
         echo \"acme conventions apply\"   (new, from company)
       mcp fixture  stdio
         npx -y @acme/fixture-mcp        (new, from company)
       review the change, then re-run with `--accept-exec`";

const LINTER_WARNING: &str =
    "warning: mcp \"linter\" connects to https://mcp.invalid/fixture (new, from company)";

#[test]
fn refuses_a_first_install_from_a_review_catalog_and_writes_nothing() {
    let project = configured(Some("review"));
    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Drift);
    assert_eq!(result.stderr, FIRST_INSTALL_REFUSAL);
    assert!(!project.exists(LOCK_FILENAME));
    assert!(!project.exists(".agents"));
}

#[test]
fn refuses_a_dry_run_the_same_way_the_install_would() {
    let project = configured(Some("review"));
    let result = project.cli(&["install", "--dry-run"]);

    assert_eq!(result.code, ExitCode::Drift);
    assert_eq!(result.stderr, FIRST_INSTALL_REFUSAL);
}

#[test]
fn installs_what_was_accepted_and_passes_the_next_install_without_the_flag() {
    let project = configured(Some("review"));
    let accepted = project.cli(&["install", "--accept-exec"]);

    assert_eq!(accepted.code, ExitCode::Success, "{}", accepted.stderr);
    assert_eq!(accepted.stderr, LINTER_WARNING);
    assert!(project.read(LOCK_FILENAME).contains("    exec: sha256-"));

    let again = project.cli(&["install"]);

    assert_eq!(again.code, ExitCode::Success, "{}", again.stderr);
    assert_eq!(again.stderr, "");
}

#[test]
fn refuses_a_changed_command_and_a_new_hook_after_an_accepted_install() {
    let project = configured(Some("review"));

    assert_eq!(
        project.cli(&["install", "--accept-exec"]).code,
        ExitCode::Success
    );

    let notes = project.read_catalog("hooks/session-notes/hook.yml");

    project.write_catalog(
        "hooks/session-notes/hook.yml",
        &notes.replace(
            "acme conventions apply",
            "acme conventions apply; curl evil",
        ),
    );
    project.write_catalog(
        "hooks/block-rm/hook.yml",
        "name: block-rm\nevent: Stop\ntype: command\ncommand: rm -rf /tmp/x\n",
    );

    let config = project.read("ambit.yml");

    project.write(
        "ambit.yml",
        &format!("{config}  - {{ hook: \"{CATALOG_NAME}/block-rm\" }}\n"),
    );

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Drift);
    assert_eq!(
        result.stderr,
        "\
error: install would add execution that was not in the lock
       hook block-rm  Stop
         rm -rf /tmp/x                              (new, from company)
       hook session-notes  SessionStart
         echo \"acme conventions apply; curl evil\"   (command changed)
       review the change, then re-run with `--accept-exec`"
    );
}

#[test]
fn installs_a_path_catalog_without_review_unless_it_asks_for_it() {
    for trust in [None, Some("full")] {
        let project = configured(trust);
        let result = project.cli(&["install"]);

        assert_eq!(
            result.code,
            ExitCode::Success,
            "{trust:?}: {}",
            result.stderr
        );
        assert_eq!(result.stderr, "", "{trust:?}");
    }
}

#[test]
fn warns_of_a_new_http_server_without_refusing_it() {
    let project = Project::new();

    project.write(
        "ambit.yml",
        &format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\n    trust: review\nrequires:\n  - {{ mcp: \"{CATALOG_NAME}/{PACKED_MCP}\" }}\n"
        ),
    );

    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(result.stderr, LINTER_WARNING);
}

#[test]
fn leaves_a_frozen_install_to_the_frozen_check() {
    let project = configured(Some("review"));
    let refused = project.cli(&["install", "--frozen"]);

    // No lock: `--frozen` answers first, and the gate never runs.
    assert_eq!(refused.code, ExitCode::Drift);
    assert!(
        refused
            .stderr
            .starts_with("error: ambit.lock is out of date"),
        "{}",
        refused.stderr
    );

    assert_eq!(
        project.cli(&["install", "--accept-exec"]).code,
        ExitCode::Success
    );

    let frozen = project.cli(&["install", "--frozen"]);

    assert_eq!(frozen.code, ExitCode::Success, "{}", frozen.stderr);
    assert_eq!(frozen.stderr, "");
}
