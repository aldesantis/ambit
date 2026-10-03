//! `ambit export` end to end, against a catalog written into a temporary project.
//!
//! The end-to-end cases need the config and catalog loaders (B1) and resolution (B2); the lock case
//! also needs the lock writer (B4). The unit cases at the bottom pin the file collection, layout and
//! comparison this module owns, and run on their own.
#![allow(clippy::disallowed_methods)] // std::fs::read_dir lists the output in the OS's order.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::files::{PackageFile, add_file, collect_files};
use super::tree::{ExportEntry, UNSUPPORTED_MODE};
use super::*;
use crate::errors::{AmbitError, ExitCode};
use crate::model::catalog::{CatalogLoadOptions, load_catalogs, merge_catalogs};
use crate::model::config::load_project_config;
use crate::project::lock::{build_lock, serialize_lock};
use crate::resolution::resolve::resolve_bundle;
use crate::test_support::{run_cli, tempdir, test_env};
use crate::util::env::Env;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    env: Env,
}

impl Fixture {
    fn context(&self) -> SourceContext {
        SourceContext {
            project_dir: self.source.clone(),
            env: self.env.clone(),
            offline: true,
        }
    }

    fn put(&self, file: &str, value: &str) {
        let target = self.source.join(file);

        std::fs::create_dir_all(target.parent().expect("a parent")).expect("create a directory");
        std::fs::write(target, value).expect("write a file");
    }

    fn read(&self, file: &str) -> String {
        std::fs::read_to_string(self.root.join("out").join(file)).expect("read an exported file")
    }

    fn json(&self, file: &str) -> serde_json::Value {
        serde_json::from_str(&self.read(file)).expect("parse an exported file")
    }

    fn out(&self) -> String {
        self.root.join("out").to_string_lossy().into_owned()
    }

    fn export_it(&self) -> Result<ExportResult> {
        export_plugins(
            &self.context(),
            &ExportOptions {
                output: self.out(),
                ..ExportOptions::default()
            },
        )
    }

    #[allow(clippy::needless_pass_by_value)] // Callers build the options inline.
    fn export(&self, options: ExportOptions) -> Result<ExportResult> {
        export_plugins(&self.context(), &options)
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("list a directory")
        .map(|entry| {
            entry
                .expect("an entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn expect_error(result: Result<ExportResult>, diagnostic: &str) -> AmbitError {
    let error = result.expect_err("expected the export to be refused");

    assert!(
        error.format().contains(diagnostic),
        "expected {diagnostic:?} in:\n{}",
        error.format()
    );
    error
}

fn fixture() -> Fixture {
    let dir = tempdir();
    let root = crate::util::fs::canonicalize(dir.path()).expect("resolve the tempdir");
    let source = root.join("source");
    let fixture = Fixture {
        env: test_env(&root),
        _dir: dir,
        root,
        source,
    };

    fixture.put(
        "ambit.yml",
        "version: 1\ncatalogs: [{name: local, source: 'path:.'}]\nrequires: [{pack: local/work}]\n",
    );
    fixture.put(
        "packs/work.yml",
        "name: work\nplugin:\n  name: example-work\n  version: 1.2.3\n  description: Work tools\n  author: {name: Example}\n  directory: work\n  dependencies: [external-tools]\nrequires:\n  - skill: do-work\n  - pack: base\n  - hook: check\n  - mcp: api\n",
    );
    fixture.put(
        "packs/base.yml",
        "name: base\nplugin: {name: example-base}\nrequires: [{skill: use-base}]\n",
    );
    fixture.put(
        "skills/do-work/SKILL.md",
        "---\nname: do-work\ndescription: Do work\nambit:\n  requires: [{skill: helper}]\n---\nUse /example-base:use-base.\n",
    );
    fixture.put(
        "skills/helper/SKILL.md",
        "---\nname: helper\ndescription: Help\n---\nHelp.\n",
    );
    fixture.put(
        "skills/use-base/SKILL.md",
        "---\nname: use-base\ndescription: Base\n---\nBase.\n",
    );
    fixture.put(
        "hooks/check/hook.yml",
        "name: check\nevent: PreToolUse\nmatcher: Bash\ntype: script\ncommand: check.sh --flag\ntimeout: 5\n",
    );
    fixture.put(
        "hooks/check/check.sh",
        "#!/bin/sh\ncat \"$(dirname \"$0\")/message.txt\"\n",
    );
    set_mode(&fixture.source.join("hooks/check/check.sh"), 0o755);
    fixture.put("hooks/check/message.txt", "relocated successfully\n");
    fixture.put(
        "mcps/api.yml",
        "name: api\ntransport:\n  http:\n    url: https://example.test/mcp\n    bearer_token_env_var: EXPORT_TEST_TOKEN\n",
    );

    fixture
}

fn set_mode(target: &Path, mode: u32) {
    super::set_mode(target, mode).expect("chmod");
}

#[cfg(unix)]
fn mode(target: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::metadata(target)
        .expect("stat")
        .permissions()
        .mode()
        & 0o777
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create a symlink");
}

#[test]
fn preserves_plugin_boundaries_external_dependencies_and_transitive_skill_requirements() {
    let f = fixture();
    f.export_it().expect("export");

    assert_eq!(names(&f.root.join("out")), ["example-base", "work"]);
    assert_eq!(
        f.json("work/.claude-plugin/plugin.json"),
        serde_json::json!({
            "name": "example-work",
            "version": "1.2.3",
            "description": "Work tools",
            "author": { "name": "Example" },
            "dependencies": ["external-tools", "example-base"],
        })
    );
    assert_eq!(
        names(&f.root.join("out/work/skills")),
        ["do-work", "helper"]
    );
    assert_eq!(
        f.read("work/skills/do-work/SKILL.md"),
        std::fs::read_to_string(f.source.join("skills/do-work/SKILL.md")).expect("read")
    );
    assert_eq!(
        f.json("work/.mcp.json"),
        serde_json::json!({
            "mcpServers": {
                "api": {
                    "type": "http",
                    "url": "https://example.test/mcp",
                    "headers": { "Authorization": "Bearer ${EXPORT_TEST_TOKEN}" },
                },
            },
        })
    );
    assert_eq!(
        f.json("work/hooks/hooks.json"),
        serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "Bash",
                        "hooks": [
                            {
                                "type": "command",
                                "command": "${CLAUDE_PLUGIN_ROOT}/hooks/check.sh --flag",
                                "timeout": 5,
                            },
                        ],
                    },
                ],
            },
        })
    );
    assert_eq!(
        names(&f.root.join("out/work/hooks")),
        ["check.sh", "hooks.json", "message.txt"]
    );
    assert!(!names(&f.source).contains(&".ambit".to_owned()));
    assert!(!names(&f.source).contains(&"ambit.lock".to_owned()));
}

#[cfg(unix)]
#[test]
fn copies_symlinked_assets_preserves_executability_and_runs_after_relocation_and_source_removal() {
    let f = fixture();
    symlink(
        "../../hooks/check/message.txt",
        &f.source.join("skills/do-work/message.txt"),
    );
    f.export_it().expect("export");
    let exported = f.root.join("out/work");

    assert!(!exported.join("skills/do-work/message.txt").is_symlink());
    assert_eq!(mode(&exported.join("hooks/check.sh")), 0o755);

    std::fs::rename(f.root.join("out"), f.root.join("moved")).expect("move the export");
    std::fs::remove_dir_all(&f.source).expect("remove the source");

    let output = Command::new(f.root.join("moved/work/hooks/check.sh"))
        .current_dir(std::env::temp_dir())
        .output()
        .expect("run the hook");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "relocated successfully\n"
    );
}

#[test]
fn exports_dependency_only_packs_and_expands_helper_packs_without_plugin_metadata() {
    let f = fixture();
    f.put(
        "packs/work.yml",
        "name: work\nplugin: {name: example-work}\nrequires: [{pack: helper}]\n",
    );
    f.put(
        "packs/helper.yml",
        "name: helper\nrequires: [{pack: base}]\n",
    );
    f.export_it().expect("export");

    assert_eq!(
        f.json("example-work/.claude-plugin/plugin.json"),
        serde_json::json!({ "name": "example-work", "dependencies": ["example-base"] })
    );
    assert_eq!(names(&f.root.join("out/example-work")), [".claude-plugin"]);
}

#[test]
fn keeps_dependency_declaration_order_and_removes_duplicates() {
    let f = fixture();
    f.put("packs/z.yml", "name: z\nplugin: {name: example-z}\n");
    f.put(
        "packs/work.yml",
        "name: work\nplugin: {name: example-work}\nrequires: [{pack: z}, {pack: base}, {pack: z}]\n",
    );
    f.export_it().expect("export");

    assert_eq!(
        f.json("example-work/.claude-plugin/plugin.json")["dependencies"],
        serde_json::json!(["example-z", "example-base"])
    );
}

#[test]
fn copies_claude_command_files_without_leaking_catalog_paths_into_the_manifest() {
    let f = fixture();
    f.put(
        "packs/work.yml",
        "name: work\nplugin: {name: example-work, commands: commands/work}\n",
    );
    let command = "---\ndescription: A command\n---\nCommand body.\n";

    f.put("commands/work/check.md", command);
    f.export_it().expect("export");

    assert_eq!(f.read("example-work/commands/check.md"), command);
    assert_eq!(
        f.json("example-work/.claude-plugin/plugin.json"),
        serde_json::json!({ "name": "example-work" })
    );
}

#[test]
fn validates_a_dry_run_without_writing_output_or_its_parent() {
    let f = fixture();
    let result = f
        .export(ExportOptions {
            output: "missing/deep/output".to_owned(),
            dry_run: true,
            ..ExportOptions::default()
        })
        .expect("a dry run");

    assert_eq!(result.plugins.len(), 2);
    assert!(!names(&f.source).contains(&"missing".to_owned()));
}

const REJECTIONS: &[(&str, &str, &str, &str)] = &[
    (
        "missing plugin metadata",
        "packs/work.yml",
        "name: work\n",
        "no plugin metadata",
    ),
    (
        "unsafe output directory",
        "packs/work.yml",
        "name: work\nplugin: {name: good, directory: ../escape}\n",
        "basename",
    ),
    (
        "unknown plugin field",
        "packs/work.yml",
        "name: work\nplugin: {name: good, typo: true}\n",
        "unknown key",
    ),
    (
        "invalid plugin name",
        "packs/work.yml",
        "name: work\nplugin: {name: Bad_Name}\n",
        "plugin names",
    ),
    (
        "unsafe commands path",
        "packs/work.yml",
        "name: work\nplugin: {name: good, commands: ../escape}\n",
        "inside the catalog",
    ),
    (
        "missing skill description",
        "skills/helper/SKILL.md",
        "---\nname: helper\n---\nHelp\n",
        "description",
    ),
    (
        "missing requirement",
        "packs/base.yml",
        "name: base\nplugin: {name: example-base}\nrequires: [{skill: missing}]\n",
        "matches nothing",
    ),
    (
        "dependency cycle",
        "packs/base.yml",
        "name: base\nplugin: {name: example-base}\nrequires: [{pack: work}]\n",
        "cycle",
    ),
    (
        "duplicate plugin names",
        "packs/base.yml",
        "name: base\nplugin: {name: example-work}\n",
        "duplicate plugin",
    ),
    (
        "local MCP executable",
        "mcps/api.yml",
        "name: api\ntransport: {stdio: {command: ./server.js}}\n",
        "local file",
    ),
    (
        "unbundled hook",
        "hooks/check/hook.yml",
        "name: check\nevent: Stop\ntype: command\ncommand: ./local.sh\n",
        "local file",
    ),
    (
        "missing namespace dependency",
        "skills/do-work/SKILL.md",
        "---\nname: do-work\ndescription: Do work\n---\nUse /unknown:missing.\n",
        "unavailable skill",
    ),
    (
        "missing namespaced skill",
        "skills/do-work/SKILL.md",
        "---\nname: do-work\ndescription: Do work\n---\nUse /example-base:missing.\n",
        "unavailable skill",
    ),
];

#[test]
fn rejects_each_invalid_package_before_creating_output() {
    for (label, file, content, diagnostic) in REJECTIONS {
        let f = fixture();
        f.put(file, content);

        let result = f.export_it();

        assert!(result.is_err(), "{label}: expected a refusal");
        expect_error(result, diagnostic);
        assert!(!names(&f.root).contains(&"out".to_owned()), "{label}");
    }
}

#[test]
fn rejects_a_nested_skill_layout() {
    let f = fixture();
    f.put(
        "skills/do-work/nested/SKILL.md",
        "---\nname: do-work.nested\ndescription: Nested\n---\nNested.\n",
    );

    expect_error(f.export_it(), "nested skill");
}

#[cfg(unix)]
#[test]
fn rejects_catalog_escaping_and_cyclic_symlinks() {
    let f = fixture();
    std::fs::write(f.root.join("secret"), "not a plugin asset").expect("write");
    let link = f.source.join("skills/helper/link");

    symlink(&f.root.join("secret").to_string_lossy(), &link);
    expect_error(f.export_it(), "escapes its catalog");
    std::fs::remove_file(&link).expect("remove the link");
    symlink(".", &link);
    expect_error(f.export_it(), "cyclic asset");
}

#[test]
fn rejects_a_hook_asset_colliding_with_the_generated_config() {
    let f = fixture();
    f.put("hooks/check/hooks.json", "{}");

    expect_error(f.export_it(), "path collision");
}

#[test]
fn refuses_an_existing_output_without_changing_it() {
    let f = fixture();
    std::fs::create_dir(f.root.join("out")).expect("create the output");
    std::fs::write(f.root.join("out/keep"), "keep").expect("write");

    expect_error(f.export_it(), "already exists");
    assert_eq!(f.read("keep"), "keep");
}

#[test]
fn requires_explicit_cli_format_and_output_and_supports_json_dry_runs() {
    let f = fixture();
    let cli = |args: &[&str]| run_cli(args, &f.source, &f.env);

    assert_eq!(cli(&["export"]).code, ExitCode::Config);
    assert_eq!(
        cli(&["export", "--format", "agent-plugin", "--output", "out"]).code,
        ExitCode::Config
    );

    let result = cli(&[
        "export",
        "--format",
        "claude-plugin",
        "--output",
        "out",
        "--dry-run",
        "--json",
    ]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    let parsed: serde_json::Value = serde_json::from_str(&result.stdout).expect("JSON");

    assert_eq!(parsed["plugins"].as_array().map(Vec::len), Some(2));
    assert!(!names(&f.source).contains(&"out".to_owned()));
}

#[test]
fn uses_locked_git_revisions_reproducibly_including_metadata_when_offline() {
    let f = fixture();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(&f.source)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .expect("run git");

        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    };
    let commit_all = |message: &str| {
        git(&["add", "."]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "commit",
            "-m",
            message,
        ]);
    };

    git(&["init", "--quiet"]);
    git(&["symbolic-ref", "HEAD", "refs/heads/main"]);
    commit_all("Initial catalog");
    let commit = git(&["rev-parse", "HEAD"]);
    let project = f.root.join("consumer");

    std::fs::create_dir(&project).expect("create the consumer");
    std::fs::write(
        project.join("ambit.yml"),
        format!(
            "version: 1\ncatalogs: [{{name: local, source: '{}'}}]\nrequires: [{{pack: local/work}}]\n",
            crate::test_support::fixture_catalog::file_url(&f.source)
        ),
    )
    .expect("write ambit.yml");

    let remote = SourceContext {
        project_dir: project.clone(),
        env: f.env.clone(),
        offline: false,
    };
    let config = load_project_config(&project).expect("load the config");
    let catalogs = load_catalogs(&config, &remote, &mut CatalogLoadOptions::default())
        .expect("load the catalogs");
    let bundle = resolve_bundle(&config, &merge_catalogs(&catalogs)).expect("resolve");
    let lock = serialize_lock(&build_lock(&catalogs, &bundle).expect("build the lock"));

    std::fs::write(project.join("ambit.lock"), &lock).expect("write the lock");
    assert_eq!(catalogs[0].commit.as_deref(), Some(commit.as_str()));

    let first = f.root.join("first").to_string_lossy().into_owned();
    export_plugins(
        &remote,
        &ExportOptions {
            output: first,
            ..ExportOptions::default()
        },
    )
    .expect("the first export");

    f.put(
        "packs/work.yml",
        "name: work\nplugin: {name: changed-name}\n",
    );
    commit_all("Change metadata");
    std::fs::remove_dir_all(&f.source).expect("remove the source");

    let second = f.root.join("second").to_string_lossy().into_owned();
    export_plugins(
        &SourceContext {
            offline: true,
            ..remote
        },
        &ExportOptions {
            output: second,
            ..ExportOptions::default()
        },
    )
    .expect("the second export");

    let compare = |relative: &str| std::fs::read_to_string(f.root.join(relative)).expect("read");

    assert_eq!(
        compare("second/work/.claude-plugin/plugin.json"),
        compare("first/work/.claude-plugin/plugin.json")
    );
    assert_eq!(
        compare("second/work/skills/do-work/SKILL.md"),
        compare("first/work/skills/do-work/SKILL.md")
    );
    assert_eq!(
        std::fs::read_to_string(project.join("ambit.lock")).expect("read the lock"),
        lock
    );
}

#[test]
fn preserves_empty_supporting_directories() {
    let f = fixture();
    std::fs::create_dir_all(f.source.join("skills/helper/assets/empty")).expect("create");
    f.export_it().expect("export");

    assert!(f.root.join("out/work/skills/helper/assets/empty").is_dir());
}

#[test]
fn rejects_malformed_claude_frontmatter_and_links_above_the_plugin_root() {
    let f = fixture();
    f.put(
        "skills/helper/SKILL.md",
        "---\nname: helper\ndescription: Help\nuser-invocable: 'no'\n---\nHelp.\n",
    );
    expect_error(f.export_it(), "boolean");
    f.put(
        "skills/helper/SKILL.md",
        "---\nname: helper\ndescription: Help\n---\n[Outside](../../../outside.md)\n",
    );
    expect_error(f.export_it(), "escapes the plugin");
}

#[test]
fn rejects_missing_assets_referenced_through_claude_plugin_root() {
    let f = fixture();
    f.put(
        "mcps/api.yml",
        "name: api\ntransport: {stdio: {command: node, args: ['${CLAUDE_PLUGIN_ROOT}/missing.js']}}\n",
    );

    expect_error(f.export_it(), "missing or escapes");
}

#[test]
fn rejects_an_mcp_script_argument_that_would_depend_on_the_source_checkout() {
    let f = fixture();
    f.put(
        "mcps/api.yml",
        "name: api\ntransport: {stdio: {command: node, args: ['./server.js']}}\n",
    );

    expect_error(f.export_it(), "local path");
}

#[test]
fn requires_a_directory_for_slash_commands_and_a_valid_homepage_url() {
    let f = fixture();
    f.put(
        "packs/work.yml",
        "name: work\nplugin: {name: good, homepage: not-a-url}\n",
    );
    expect_error(f.export_it(), "invalid URL");
    f.put(
        "packs/work.yml",
        "name: work\nplugin: {name: good, commands: command.md}\n",
    );
    f.put("command.md", "---\ndescription: Command\n---\nBody\n");
    expect_error(f.export_it(), "asset directory");
}

fn linked(output: &str) -> ExportOptions {
    ExportOptions {
        output: output.to_owned(),
        link: true,
        ..ExportOptions::default()
    }
}

#[cfg(unix)]
#[test]
fn links_skills_and_hook_assets_relative_to_the_final_output_and_survives_moving_the_repository() {
    let f = fixture();
    f.export(linked("plugins")).expect("export");
    let plugin = f.source.join("plugins/work");

    assert_eq!(
        std::fs::read_link(plugin.join("skills/do-work")).expect("a link"),
        Path::new("../../../skills/do-work")
    );
    assert_eq!(
        std::fs::read_link(plugin.join("hooks/check.sh")).expect("a link"),
        Path::new("../../../hooks/check/check.sh")
    );
    assert!(!plugin.join("hooks/hooks.json").is_symlink());

    let moved = f.root.join("moved");

    std::fs::rename(&f.source, &moved).expect("move the repository");
    assert!(
        std::fs::read_to_string(moved.join("plugins/work/skills/do-work/SKILL.md"))
            .expect("read through the link")
            .contains("Do work")
    );

    let output = Command::new("sh")
        .arg(moved.join("plugins/work/hooks/check.sh"))
        .output()
        .expect("run the hook");

    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "relocated successfully\n"
    );
}

#[test]
fn rejects_remote_catalogs_for_linked_exports_even_during_dry_runs() {
    let f = fixture();
    f.put(
        "ambit.yml",
        "version: 1\ncatalogs: [{name: remote, source: 'github:example/catalog'}]\nrequires: [{pack: remote/work}]\n",
    );

    expect_error(
        f.export(ExportOptions {
            dry_run: true,
            ..linked("plugins")
        }),
        "linked exports require local path catalogs",
    );
}

#[cfg(unix)]
#[test]
fn checks_linked_exports_without_writing_and_replaces_drift_while_retaining_json_formatting() {
    let f = fixture();
    let check = || ExportOptions {
        check: true,
        ..linked("plugins")
    };

    f.export(linked("plugins")).expect("export");
    let manifest = f.source.join("plugins/work/.claude-plugin/plugin.json");
    let original: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest).expect("read")).expect("JSON");
    let formatted = serde_json::to_string(&original).expect("serialize");

    std::fs::write(&manifest, &formatted).expect("reformat the manifest");
    let before = std::fs::metadata(&manifest)
        .and_then(|m| m.modified())
        .expect("mtime");

    f.export(check()).expect("no drift");
    assert_eq!(
        std::fs::metadata(&manifest)
            .and_then(|m| m.modified())
            .expect("mtime"),
        before
    );

    f.put("plugins/stale/file", "stale");
    assert_eq!(f.export(check()).expect_err("drift").code, ExitCode::Drift);
    assert_eq!(
        std::fs::read_to_string(f.source.join("plugins/stale/file")).expect("read"),
        "stale"
    );

    f.export(ExportOptions {
        force: true,
        ..linked("plugins")
    })
    .expect("replace");
    assert_eq!(std::fs::read_to_string(&manifest).expect("read"), formatted);
    assert_eq!(
        std::fs::read_link(f.source.join("plugins/work/skills/do-work")).expect("a link"),
        Path::new("../../../skills/do-work")
    );
    assert!(!f.source.join("plugins/stale").exists());

    f.export(check()).expect("no drift");

    let mut changed = original.clone();
    changed["version"] = serde_json::json!("2.0.0");
    std::fs::write(&manifest, changed.to_string()).expect("change the manifest");
    assert_eq!(f.export(check()).expect_err("drift").code, ExitCode::Drift);
}

#[cfg(unix)]
#[test]
fn detects_changed_link_targets_and_copied_directories_in_linked_exports() {
    let f = fixture();
    let check = ExportOptions {
        check: true,
        ..linked("plugins")
    };

    f.export(linked("plugins")).expect("export");
    let skill = f.source.join("plugins/work/skills/do-work");

    std::fs::remove_file(&skill).expect("remove the link");
    symlink("../../../skills/./do-work", &skill);
    assert_eq!(
        f.export(check.clone()).expect_err("drift").code,
        ExitCode::Drift
    );

    std::fs::remove_file(&skill).expect("remove the link");
    std::fs::create_dir(&skill).expect("create a directory");
    std::fs::copy(
        f.source.join("skills/do-work/SKILL.md"),
        skill.join("SKILL.md"),
    )
    .expect("copy");
    assert_eq!(f.export(check).expect_err("drift").code, ExitCode::Drift);
}

#[cfg(unix)]
#[test]
fn detects_changed_bytes_and_executable_permissions_in_standalone_exports() {
    let f = fixture();
    f.export_it().expect("export");
    let check = ExportOptions {
        output: f.out(),
        check: true,
        ..ExportOptions::default()
    };

    f.export(check.clone()).expect("no drift");
    let script = f.root.join("out/work/hooks/check.sh");

    set_mode(&script, 0o644);
    assert_eq!(
        f.export(check.clone()).expect_err("drift").code,
        ExitCode::Drift
    );
    set_mode(&script, 0o755);
    std::fs::write(&script, "changed").expect("write");
    assert_eq!(f.export(check).expect_err("drift").code, ExitCode::Drift);
}

#[cfg(unix)]
#[test]
fn refuses_unsafe_replacements_and_validates_before_touching_an_existing_export() {
    let f = fixture();
    let forced = |output: &str| ExportOptions {
        output: output.to_owned(),
        force: true,
        ..ExportOptions::default()
    };

    expect_error(f.export(forced(".")), "contains source files");
    expect_error(f.export(forced("skills")), "contains source files");
    expect_error(
        f.export(forced("skills/do-work/assets")),
        "overlaps source assets",
    );
    symlink(&f.source.to_string_lossy(), &f.root.join("alias"));
    expect_error(
        f.export(forced(&f.root.join("alias").to_string_lossy())),
        "regular directory",
    );

    f.export_it().expect("export");
    let manifest = f.read("work/.claude-plugin/plugin.json");

    f.put("skills/helper/SKILL.md", "missing frontmatter");
    assert!(f.export(forced(&f.out())).is_err());
    assert_eq!(f.read("work/.claude-plugin/plugin.json"), manifest);
}

#[test]
fn exposes_check_and_force_through_the_cli_and_leaves_missing_output_untouched() {
    let f = fixture();
    let cli = |flags: &[&str]| {
        let mut args = vec![
            "export",
            "--format",
            "claude-plugin",
            "--output",
            "missing/plugins",
            "--link",
        ];
        args.extend(flags);
        run_cli(&args, &f.source, &f.env).code
    };

    assert_eq!(cli(&["--check"]), ExitCode::Drift);
    assert!(!names(&f.source).contains(&"missing".to_owned()));
    assert_eq!(cli(&["--force", "--dry-run"]), ExitCode::Success);
    assert!(!names(&f.source).contains(&"missing".to_owned()));
    assert_eq!(cli(&["--force"]), ExitCode::Success);
    assert_eq!(cli(&["--check"]), ExitCode::Success);
    assert_eq!(cli(&["--check", "--force"]), ExitCode::Config);
    assert_eq!(cli(&["--check", "--dry-run"]), ExitCode::Config);
}

// Units this module owns, runnable without the loaders.

#[test]
fn refuses_check_combined_with_force_or_dry_run_before_reading_anything() {
    let dir = tempdir();
    let context = SourceContext {
        project_dir: dir.path().to_path_buf(),
        ..SourceContext::default()
    };

    for (force, dry_run) in [(true, false), (false, true)] {
        let error = export_plugins(
            &context,
            &ExportOptions {
                output: "out".to_owned(),
                check: true,
                force,
                dry_run,
                ..ExportOptions::default()
            },
        )
        .expect_err("a refusal");

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(
            error.message,
            "--check cannot be combined with --force or --dry-run"
        );
    }
}

#[test]
fn refuses_an_existing_output_before_reading_the_config() {
    let dir = tempdir();
    std::fs::create_dir(dir.path().join("out")).expect("create the output");
    let context = SourceContext {
        project_dir: dir.path().to_path_buf(),
        ..SourceContext::default()
    };
    let error = export_plugins(
        &context,
        &ExportOptions {
            output: "out".to_owned(),
            ..ExportOptions::default()
        },
    )
    .expect_err("a refusal");

    assert_eq!(error.code, ExitCode::Config);
    assert!(error.message.starts_with("export output already exists: "));
    assert_eq!(
        error.detail,
        ["use --force to regenerate it or --check to check for drift"]
    );
}

#[test]
fn refuses_a_package_path_collision() {
    let mut files = PackageFiles::new();

    add_file(&mut files, "hooks/a.sh", b"a".to_vec(), 0o644).expect("add");

    let error = add_file(&mut files, "hooks/a.sh", b"b".to_vec(), 0o644).expect_err("a collision");

    assert_eq!(error.message, "export path collision at hooks/a.sh");
    assert!(add_file(&mut files, "hooks", Vec::new(), 0o644).is_err());
    assert!(add_file(&mut files, "hooks/b.sh", Vec::new(), 0o644).is_ok());
}

#[cfg(unix)]
#[test]
fn collects_an_asset_directory_dereferencing_links_inside_the_catalog() {
    let dir = tempdir();
    let catalog = crate::util::fs::canonicalize(dir.path()).expect("resolve");
    let skill = catalog.join("skills/one");

    std::fs::create_dir_all(skill.join("empty")).expect("create");
    std::fs::write(skill.join("SKILL.md"), "body").expect("write");
    std::fs::write(catalog.join("shared.txt"), "shared").expect("write");
    std::fs::write(skill.join("hook.yml"), "excluded").expect("write");
    symlink("../../shared.txt", &skill.join("linked.txt"));

    let mut files = PackageFiles::new();
    collect_files(
        &mut files,
        &skill,
        "skills/one",
        &catalog,
        &["hook.yml".to_owned()],
    )
    .expect("collect");

    assert_eq!(
        files.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "skills/one",
            "skills/one/SKILL.md",
            "skills/one/empty",
            "skills/one/linked.txt",
        ]
    );
    assert_eq!(
        files["skills/one/linked.txt"],
        PackageFile {
            data: Some(b"shared".to_vec()),
            mode: mode(&catalog.join("shared.txt")),
            source: Some(catalog.join("shared.txt")),
        }
    );
    assert_eq!(files["skills/one/empty"].data, None);
}

#[cfg(unix)]
#[test]
fn refuses_links_leaving_the_catalog_and_cycles() {
    let dir = tempdir();
    let root = crate::util::fs::canonicalize(dir.path()).expect("resolve");
    let catalog = root.join("catalog");
    let skill = catalog.join("skills/one");

    std::fs::create_dir_all(&skill).expect("create");
    std::fs::write(root.join("secret"), "secret").expect("write");
    symlink(&root.join("secret").to_string_lossy(), &skill.join("link"));

    let error =
        collect_files(&mut PackageFiles::new(), &skill, "s", &catalog, &[]).expect_err("an escape");

    assert!(error.message.ends_with("link: asset escapes its catalog"));

    std::fs::remove_file(skill.join("link")).expect("remove");
    symlink(".", &skill.join("link"));

    let error =
        collect_files(&mut PackageFiles::new(), &skill, "s", &catalog, &[]).expect_err("a cycle");

    assert!(error.message.ends_with("link: cyclic asset symlink"));

    let error = collect_files(
        &mut PackageFiles::new(),
        &skill.join("missing"),
        "s",
        &catalog,
        &[],
    )
    .expect_err("a missing directory");

    assert_eq!(error.code, ExitCode::Config);
    assert_eq!(error.message, "cannot export Claude plugins");
    assert!(error.detail[0].starts_with("ENOENT: no such file or directory, stat '"));
}

fn file(data: &[u8], mode: u32, source: Option<&str>) -> PackageFile {
    PackageFile {
        data: Some(data.to_vec()),
        mode,
        source: source.map(PathBuf::from),
    }
}

fn directory(source: Option<&str>) -> PackageFile {
    PackageFile {
        data: None,
        mode: 0o755,
        source: source.map(PathBuf::from),
    }
}

#[cfg(unix)]
#[test]
fn lays_out_packages_linking_skill_directories_and_hook_files_when_asked() {
    let files: PackageFiles = [
        (
            ".claude-plugin/plugin.json".to_owned(),
            file(b"{}", 0o644, None),
        ),
        ("skills/one".to_owned(), directory(Some("/repo/skills/one"))),
        (
            "skills/one/SKILL.md".to_owned(),
            file(b"x", 0o644, Some("/repo/skills/one/SKILL.md")),
        ),
        (
            "hooks/run.sh".to_owned(),
            file(b"#!", 0o755, Some("/repo/hooks/h/run.sh")),
        ),
        ("hooks/hooks.json".to_owned(), file(b"{}", 0o644, None)),
    ]
    .into_iter()
    .collect();
    let packages = [TreePackage {
        directory: "work".to_owned(),
        files,
    }];

    let copied = package_tree(&packages, Path::new("/repo/plugins"), false);

    assert_eq!(
        copied.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "work",
            "work/.claude-plugin",
            "work/.claude-plugin/plugin.json",
            "work/skills",
            "work/skills/one",
            "work/skills/one/SKILL.md",
            "work/hooks",
            "work/hooks/run.sh",
            "work/hooks/hooks.json",
        ]
    );

    let linked = package_tree(&packages, Path::new("/repo/plugins"), true);

    assert_eq!(
        linked["work/skills/one"].link.as_deref(),
        Some("../../../skills/one")
    );
    assert_eq!(linked["work/skills/one"].link_type, Some(LinkType::Dir));
    assert!(!linked.contains_key("work/skills/one/SKILL.md"));
    assert_eq!(
        linked["work/hooks/run.sh"].link.as_deref(),
        Some("../../../hooks/h/run.sh")
    );
    assert_eq!(linked["work/hooks/run.sh"].link_type, Some(LinkType::File));
    assert_eq!(linked["work/hooks/hooks.json"].link, None);
}

fn entry(data: Option<&[u8]>, mode: u32, link: Option<&str>) -> ExportEntry {
    ExportEntry {
        data: data.map(<[u8]>::to_vec),
        mode,
        link: link.map(str::to_owned),
        link_type: None,
    }
}

#[test]
fn compares_json_by_value_and_other_files_by_bytes_and_executable_bit() {
    let pretty = entry(Some(b"{\n  \"a\": 1,\n  \"b\": [2]\n}\n"), 0o644, None);
    let compact = entry(Some(b"{\"b\":[2],\"a\":1.0}"), 0o600, None);

    assert!(same_entry("x.json", &pretty, &compact));
    assert!(!same_entry("x.txt", &pretty, &compact));
    assert!(!same_entry(
        "x.json",
        &pretty,
        &entry(Some(b"not json"), 0o644, None)
    ));
    assert!(!same_entry(
        "run.sh",
        &entry(Some(b"#!"), 0o755, None),
        &entry(Some(b"#!"), 0o644, None)
    ));
    assert!(same_entry(
        "dir",
        &entry(None, 0o755, None),
        &entry(None, 0o700, None)
    ));
    assert!(!same_entry(
        "dir",
        &entry(None, 0o755, None),
        &entry(None, UNSUPPORTED_MODE, None)
    ));
    assert!(same_entry(
        "l",
        &entry(None, 0o777, Some("../a")),
        &entry(None, 0o777, Some("../a"))
    ));
    assert!(!same_entry(
        "l",
        &entry(None, 0o777, Some("../a")),
        &entry(None, 0o777, Some("../b"))
    ));
    assert!(!same_entry(
        "l",
        &entry(None, 0o777, Some("../a")),
        &entry(None, 0o755, None)
    ));
}

#[cfg(unix)]
#[test]
fn reads_an_existing_export_without_following_links() {
    let dir = tempdir();
    let root = dir.path();

    std::fs::create_dir_all(root.join("work/skills")).expect("create");
    std::fs::write(root.join("work/a.json"), "{}").expect("write");
    symlink("../../elsewhere", &root.join("work/skills/one"));

    let tree = read_tree(root).expect("read");

    assert_eq!(
        tree.keys().map(String::as_str).collect::<Vec<_>>(),
        ["work", "work/a.json", "work/skills", "work/skills/one"]
    );
    assert_eq!(tree["work/a.json"].data.as_deref(), Some(&b"{}"[..]));
    assert_eq!(
        tree["work/skills/one"].link.as_deref(),
        Some("../../elsewhere")
    );
}

#[test]
fn resolves_the_existing_ancestors_of_a_missing_output() {
    let dir = tempdir();
    let resolved = crate::util::fs::canonicalize(dir.path()).expect("resolve");

    assert_eq!(
        canonical_path(&dir.path().join("missing/deep")).expect("a path"),
        resolved.join("missing").join("deep")
    );
}
