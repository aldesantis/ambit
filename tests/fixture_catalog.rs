//! Read through saphyr rather than ambit's YAML reader, since the fixture is what proves that
//! reader right.
#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::path::Path;

use pretty_assertions::assert_eq;
use saphyr::{LoadableYamlNode, ScalarOwned, YamlOwned};
use serde_json::{Value, json};

use support::fixture_catalog::{
    FIXTURE_MARKER, build_fixture_catalog, build_fixture_git_catalog, commit_fixture_git_revision,
    file_url, git,
};

fn list_files(dir: &Path) -> Vec<String> {
    fn walk(dir: &Path, prefix: &str, found: &mut Vec<String>) {
        for entry in fs::read_dir(dir).expect("readable dir") {
            let entry = entry.expect("readable entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };

            if entry.file_type().expect("file type").is_dir() {
                walk(&entry.path(), &relative, found);
            } else {
                found.push(relative);
            }
        }
    }

    let mut found = Vec::new();
    walk(dir, "", &mut found);
    found.sort();
    found
}

fn snapshot(dir: &Path) -> Vec<(String, String)> {
    list_files(dir)
        .into_iter()
        .map(|relative| {
            let text = fs::read_to_string(dir.join(&relative)).expect("readable file");
            (relative, text)
        })
        .collect()
}

fn to_json(node: &YamlOwned) -> Value {
    match node {
        YamlOwned::Value(ScalarOwned::Null) => Value::Null,
        YamlOwned::Value(ScalarOwned::Boolean(b)) => json!(b),
        YamlOwned::Value(ScalarOwned::Integer(i)) => json!(i),
        YamlOwned::Value(ScalarOwned::FloatingPoint(f)) => json!(f.into_inner()),
        YamlOwned::Value(ScalarOwned::String(s)) => json!(s),
        YamlOwned::Sequence(items) => Value::Array(items.iter().map(to_json).collect()),
        YamlOwned::Mapping(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let key = match to_json(key) {
                        Value::String(s) => s,
                        other => other.to_string(),
                    };
                    (key, to_json(value))
                })
                .collect(),
        ),
        other => panic!("unexpected YAML node in the fixture: {other:?}"),
    }
}

fn parse_yaml(text: &str) -> Value {
    let documents = YamlOwned::load_from_str(text).expect("fixture YAML parses");

    to_json(documents.first().expect("one document"))
}

fn read_yaml(dir: &Path, file: &str) -> Value {
    parse_yaml(&fs::read_to_string(dir.join(file)).expect("readable file"))
}

fn frontmatter(source: &str) -> Value {
    let rest = source
        .strip_prefix("---\n")
        .expect("document has no frontmatter block");
    let end = rest
        .find("\n---\n")
        .expect("document has no closing delimiter");

    parse_yaml(&rest[..end])
}

fn annotations(source: &str) -> Value {
    let block = frontmatter(source)["ambit"].clone();

    assert!(block.is_object(), "frontmatter has no `ambit:` block");
    block
}

fn expected_files() -> Vec<String> {
    let mut files: Vec<String> = [
        FIXTURE_MARKER,
        "hooks/acme-standup/hook.yml",
        "hooks/guard-secrets/hook.yml",
        "hooks/guard-secrets/guard.sh",
        "hooks/session-notes/hook.yml",
        "mcps/fixture.yml",
        "mcps/linter.yml",
        "packs/core.yml",
        "packs/function/engineering.yml",
        "packs/function/engineering/frontend.yml",
        "packs/project/acme.yml",
        "skills/company-context/SKILL.md",
        "skills/design-tokens/SKILL.md",
        "skills/code-review/SKILL.md",
        "skills/acme-brief/SKILL.md",
    ]
    .iter()
    .map(|&file| file.to_owned())
    .collect();
    files.sort();
    files
}

fn paths_where(predicate: impl Fn(&str) -> bool) -> Vec<String> {
    expected_files()
        .into_iter()
        .filter(|file| predicate(file))
        .collect()
}

fn skill_paths() -> Vec<String> {
    paths_where(|file| file.ends_with("SKILL.md"))
}

fn hook_paths() -> Vec<String> {
    paths_where(|file| file.ends_with("hook.yml"))
}

fn pack_paths() -> Vec<String> {
    paths_where(|file| file.starts_with("packs/"))
}

fn parent(document: &str) -> &str {
    document.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn name_from_path(document: &str, dirname: &str) -> String {
    let dir = parent(document);

    dir.strip_prefix(&format!("{dirname}/"))
        .unwrap_or(dir)
        .replace('/', ".")
}

fn built() -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::Builder::new()
        .prefix("ambit-fixture-")
        .tempdir()
        .expect("tempdir");
    let dir = root.path().join("catalog");

    build_fixture_catalog(&dir).expect("fixture builds");
    (root, dir)
}

#[test]
fn writes_exactly_the_expected_tree() {
    let (_root, dir) = built();

    assert_eq!(list_files(&dir), expected_files());
}

#[test]
fn carries_no_catalog_side_config_and_groups_its_items_into_packs_instead() {
    let (_root, dir) = built();
    let files = expected_files();

    assert!(!files.contains(&"scopes.yml".to_owned()));
    assert!(!files.contains(&"ambit.yml".to_owned()));

    let mut declared = Vec::new();

    for pack in pack_paths() {
        let parsed = read_yaml(&dir, &pack);

        assert!(
            parsed["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty()),
            "{pack} has no description"
        );
        declared.push(parsed["name"].as_str().expect("name").to_owned());
    }

    declared.sort();
    assert_eq!(
        declared,
        [
            "core",
            "function.engineering",
            "function.engineering.frontend",
            "project.acme"
        ]
    );
}

#[test]
fn names_every_pack_after_its_path_nested_or_flat() {
    let (_root, dir) = built();

    for pack in pack_paths() {
        let parsed = read_yaml(&dir, &pack);
        let derived = pack
            .strip_prefix("packs/")
            .and_then(|rest| rest.strip_suffix(".yml"))
            .expect("pack path")
            .replace('/', ".");

        assert_eq!(parsed["name"], json!(derived));
    }
}

#[test]
fn names_every_skill_after_its_path() {
    let (_root, dir) = built();

    for skill in skill_paths() {
        let meta = frontmatter(&fs::read_to_string(dir.join(&skill)).unwrap());

        assert_eq!(meta["name"], json!(name_from_path(&skill, "skills")));
        assert!(meta["description"].as_str().is_some_and(|d| !d.is_empty()));
    }
}

#[test]
fn names_every_hook_after_its_path() {
    let (_root, dir) = built();

    for hook in hook_paths() {
        let entity = read_yaml(&dir, &hook);

        assert_eq!(entity["name"], json!(name_from_path(&hook, "hooks")));
        assert!(
            entity["description"]
                .as_str()
                .is_some_and(|d| !d.is_empty())
        );
    }
}

#[test]
fn gathers_one_skill_into_each_pack_one_of_them_through_another_pack() {
    let (_root, dir) = built();
    let mut membership = serde_json::Map::new();

    for pack in pack_paths() {
        let parsed = read_yaml(&dir, &pack);

        membership.insert(
            parsed["name"].as_str().unwrap().to_owned(),
            parsed["requires"].clone(),
        );
    }

    let expected = json!({
        "core": [{ "skill": "company-context" }, { "hook": "session-notes" }],
        "function.engineering": [
            { "pack": "core" },
            { "skill": "code-review" },
            { "mcp": "linter" },
            { "hook": "guard-secrets" },
        ],
        "function.engineering.frontend": [
            { "pack": "function.engineering" },
            { "skill": "design-tokens" },
        ],
        "project.acme": [{ "skill": "acme-brief" }],
    });

    for (name, requires) in expected.as_object().unwrap() {
        assert_eq!(membership.get(name), Some(requires), "{name}");
    }

    assert_eq!(membership.len(), expected.as_object().unwrap().len());
}

#[test]
fn has_a_project_skill_that_reaches_a_skill_an_mcp_and_a_hook_by_requires_alone() {
    let (_root, dir) = built();
    let meta = annotations(&fs::read_to_string(dir.join("skills/acme-brief/SKILL.md")).unwrap());

    assert_eq!(
        meta["requires"],
        json!([{ "skill": "company-context" }, { "mcp": "fixture" }, { "hook": "acme-standup" }])
    );
}

#[test]
fn declares_preconditions_a_bundle_can_be_missing() {
    let (_root, dir) = built();
    let frontend =
        annotations(&fs::read_to_string(dir.join("skills/design-tokens/SKILL.md")).unwrap());

    assert_eq!(frontend["expects"], json!([{ "env": "ACME_FIGMA_TOKEN" }]));
}

#[test]
fn defines_a_requires_only_stdio_server_and_a_packed_http_server() {
    let (_root, dir) = built();
    let required = read_yaml(&dir, "mcps/fixture.yml");
    let packed = read_yaml(&dir, "mcps/linter.yml");

    assert_eq!(
        required,
        json!({
            "name": "fixture",
            "transport": { "stdio": { "command": "npx", "args": ["-y", "@acme/fixture-mcp"] } },
            "expects": [{ "env": "FIXTURE_API_KEY" }],
        })
    );
    assert_eq!(
        packed,
        json!({
            "name": "linter",
            "transport": {
                "http": {
                    "url": "https://mcp.invalid/fixture",
                    "bearer_token_env_var": "LINTER_API_KEY",
                },
            },
            "expects": [{ "env": "LINTER_API_KEY" }],
        })
    );

    for entity in [&required, &packed] {
        assert_eq!(entity["transport"].as_object().unwrap().len(), 1);
    }
}

#[test]
fn defines_an_inline_hook_a_script_shipping_hook_and_a_requires_only_hook() {
    let (_root, dir) = built();

    assert_eq!(
        read_yaml(&dir, "hooks/session-notes/hook.yml"),
        json!({
            "name": "session-notes",
            "description": "Reminds a session that Acme's conventions apply.",
            "event": "SessionStart",
            "type": "command",
            "command": "echo \"acme conventions apply\"",
        })
    );
    assert_eq!(
        read_yaml(&dir, "hooks/guard-secrets/hook.yml"),
        json!({
            "name": "guard-secrets",
            "description": "Inspects a Bash command before Acme's tooling runs it.",
            "event": "PreToolUse",
            "matcher": "Bash",
            "type": "script",
            "command": "guard.sh",
            "timeout": 10,
        })
    );
    assert_eq!(
        read_yaml(&dir, "hooks/acme-standup/hook.yml"),
        json!({
            "name": "acme-standup",
            "description": "Records what the session touched, for the Acme standup.",
            "event": "SessionEnd",
            "type": "command",
            "command": "echo \"acme session ended\"",
        })
    );
}

#[test]
fn ships_the_script_its_script_shipping_hook_names_and_only_there() {
    let (_root, dir) = built();
    let files = expected_files();
    let hooks = hook_paths();
    let shipped: Vec<&str> = hooks
        .iter()
        .map(|hook| parent(hook))
        .filter(|hook_dir| {
            files.iter().any(|file| {
                file.starts_with(&format!("{hook_dir}/")) && !file.ends_with("hook.yml")
            })
        })
        .collect();

    assert_eq!(shipped, ["hooks/guard-secrets"]);
    assert!(
        fs::read_to_string(dir.join("hooks/guard-secrets/guard.sh"))
            .unwrap()
            .contains("#!/bin/sh")
    );
}

#[cfg(unix)]
#[test]
fn ships_that_script_executable_since_a_harness_runs_it_rather_than_reading_it() {
    use std::os::unix::fs::PermissionsExt;

    let (_root, dir) = built();
    let mode = fs::symlink_metadata(dir.join("hooks/guard-secrets/guard.sh"))
        .unwrap()
        .permissions()
        .mode();

    assert_eq!(mode & 0o111, 0o111);
}

#[test]
fn names_each_mcp_entity_after_its_filename_stem() {
    let (_root, dir) = built();

    for (file, stem) in [
        ("mcps/fixture.yml", "fixture"),
        ("mcps/linter.yml", "linter"),
    ] {
        assert_eq!(read_yaml(&dir, file)["name"], json!(stem));
    }
}

#[test]
fn is_idempotent_a_rebuild_reproduces_the_tree_byte_for_byte() {
    let (_root, dir) = built();
    let before = snapshot(&dir);

    build_fixture_catalog(&dir).unwrap();

    assert_eq!(snapshot(&dir), before);
}

#[test]
fn removes_stale_files_left_by_a_previous_build() {
    let (_root, dir) = built();

    fs::write(dir.join("stale.yml"), "name: stale\n").unwrap();
    fs::create_dir_all(dir.join("skills/stale")).unwrap();
    fs::write(dir.join("skills/stale/SKILL.md"), "---\nname: stale\n---\n").unwrap();

    build_fixture_catalog(&dir).unwrap();

    assert_eq!(list_files(&dir), expected_files());
}

#[test]
fn refuses_to_overwrite_a_directory_it_did_not_create() {
    let (root, _dir) = built();
    let foreign = root.path().join("foreign");

    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("notes.md"), "mine\n").unwrap();

    let error = build_fixture_catalog(&foreign).unwrap_err();

    assert!(
        error.to_string().contains("refusing to overwrite"),
        "{error}"
    );
    assert_eq!(
        fs::read_to_string(foreign.join("notes.md")).unwrap(),
        "mine\n"
    );
}

#[test]
fn builds_into_an_existing_empty_directory() {
    let (root, _dir) = built();
    let empty = root.path().join("empty");

    fs::create_dir_all(&empty).unwrap();

    assert_eq!(build_fixture_catalog(&empty).unwrap(), empty);
    assert_eq!(list_files(&empty), expected_files());
}

fn git_root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("ambit-fixture-git-")
        .tempdir()
        .expect("tempdir")
}

fn revision(repo: &Path, reference: &str) -> String {
    git(
        &[
            "--git-dir".as_ref(),
            repo.as_os_str(),
            "rev-parse".as_ref(),
            reference.as_ref(),
        ],
        repo,
    )
    .expect("rev-parse")
}

#[test]
fn commits_the_fixture_tree_at_a_commit_two_builds_agree_on() {
    let root = git_root();
    let first = build_fixture_git_catalog(&root.path().join("a")).unwrap();
    let second = build_fixture_git_catalog(&root.path().join("b")).unwrap();

    assert_eq!(first.commit.len(), 40);
    assert!(
        first
            .commit
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    );
    assert_eq!(second.commit, first.commit);
    assert_eq!(first.url, file_url(&first.repo));
    assert_eq!(
        fs::read_to_string(first.repo.join("HEAD")).unwrap(),
        format!("ref: refs/heads/{}\n", first.branch)
    );
}

#[test]
fn moves_the_branch_and_leaves_the_tag_behind_when_a_second_revision_is_committed() {
    let root = git_root();
    let fixture = build_fixture_git_catalog(&root.path().join("a")).unwrap();

    let moved = commit_fixture_git_revision(
        &fixture,
        &[(
            "skills/second/SKILL.md",
            Some("---\nname: second\nambit:\n  tags: [core]\n---\n\n# second\n"),
        )],
        "a second revision",
    )
    .unwrap();

    assert_ne!(moved, fixture.commit);
    assert_eq!(revision(&fixture.repo, &fixture.branch), moved);
    assert_eq!(revision(&fixture.repo, &fixture.tag), fixture.commit);
}

#[test]
fn records_the_hook_script_as_executable_in_the_commit() {
    let root = git_root();
    let fixture = build_fixture_git_catalog(&root.path().join("a")).unwrap();

    let listing = git(
        &[
            "--git-dir".as_ref(),
            fixture.repo.as_os_str(),
            "ls-tree".as_ref(),
            fixture.commit.as_ref(),
            "hooks/guard-secrets/guard.sh".as_ref(),
        ],
        &fixture.repo,
    )
    .unwrap();

    assert!(listing.starts_with("100755 blob "), "{listing}");
}
