//! `ambit.lock`, and `install --frozen`.
//!
//! The lock's whole value is that identical inputs produce identical bytes, so the assertions here
//! are on the exact file rather than on a parsed view of it: a reordered key, a stray timestamp, or
//! an anchor would all survive a structural comparison and all break the diff the lock exists to
//! give.
//!
//! The commit fields need a source that has a revision, so they are covered end to end in the git
//! source tests, and here through [`build_lock`], which takes the commit as a value and therefore
//! needs no git to pin the numeric-SHA case.

use pretty_assertions::assert_eq;

use super::*;
use crate::errors::ExitCode;
use crate::model::catalog::{CatalogParseOptions, merge_catalogs, parse_catalog_directory};
use crate::model::config::load_project_config;
use crate::model::yaml::parse_yaml_mapping;
use crate::project::install::fixture::*;
use crate::resolution::resolve::resolve_bundle;

const CATALOG_SOURCE: &str = "path:../catalog";

/// A project with the default three-pack profile written.
fn project() -> Project {
    let project = Project::new();

    project.write_profile(DEFAULT_PACKS, None, &[]);
    project
}

fn read_lock(project: &Project) -> String {
    project.read(LOCK_FILENAME)
}

fn lock_exists(project: &Project) -> bool {
    crate::util::fs::read_dir_names(&project.dir)
        .expect("list the project")
        .contains(&LOCK_FILENAME.to_owned())
}

/// Adds a hook to the fixture catalog. `body` is further `hook.yml` lines; `script`, when given, is
/// written beside `hook.yml` as `(filename, contents)`, which makes `command: <filename>` a shipped
/// script rather than a command line.
fn write_catalog_hook(project: &Project, name: &str, body: &[&str], script: Option<(&str, &str)>) {
    let mut lines = vec![format!("name: {name}")];

    lines.extend(body.iter().map(|&line| line.to_owned()));
    lines.push(String::new());
    project.write_catalog(&format!("hooks/{name}/hook.yml"), &lines.join("\n"));

    if let Some((file, contents)) = script {
        project.write_catalog(&format!("hooks/{name}/{file}"), contents);
    }
}

/// The fixture catalog parsed with a commit, so the lock has one to record.
fn catalog_at(project: &Project, commit: &str) -> Catalog {
    parse_catalog_directory(
        CATALOG_NAME,
        CATALOG_SOURCE,
        &project.catalog,
        Some(commit),
        &mut CatalogParseOptions::default(),
    )
    .expect("a parsable catalog")
}

/// What the project's current profile resolves to against `catalogs`.
fn bundle_from(project: &Project, catalogs: &[Catalog]) -> Bundle {
    let config = load_project_config(&project.dir).expect("a valid config");

    resolve_bundle(&config, &merge_catalogs(catalogs)).expect("a resolvable profile")
}

// ambit.lock

#[test]
fn records_every_configured_catalog_and_every_selected_item_keys_sorted_throughout() {
    let project = project();
    let result = project.cli(&["install"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        read_lock(&project),
        [
            "catalogs:",
            &format!("  {CATALOG_NAME}:"),
            &format!("    source: {CATALOG_SOURCE}"),
            // `path` on the hook that ships a script and not on the one whose `command` is a
            // command line: a lock pins bytes, and a command line is config values.
            "hooks:",
            "  guard-secrets:",
            &format!("    catalog: {CATALOG_NAME}"),
            "    path: hooks/guard-secrets",
            "    reason: required-by:pack:function.engineering",
            "  session-notes:",
            &format!("    catalog: {CATALOG_NAME}"),
            "    reason: required-by:pack:core",
            "mcps:",
            "  linter:",
            &format!("    catalog: {CATALOG_NAME}"),
            "    reason: required-by:pack:function.engineering",
            // The packs the project named, which nothing materializes and every reason above
            // points at.
            "packs:",
            "  core:",
            &format!("    catalog: {CATALOG_NAME}"),
            &format!("    reason: pack:{CATALOG_NAME}/core"),
            "  function.engineering:",
            &format!("    catalog: {CATALOG_NAME}"),
            &format!("    reason: pack:{CATALOG_NAME}/function.engineering"),
            "  function.engineering.frontend:",
            &format!("    catalog: {CATALOG_NAME}"),
            &format!("    reason: pack:{CATALOG_NAME}/function.engineering.*"),
            "skills:",
            &format!("  {ENGINEERING_SKILL}:"),
            &format!("    catalog: {CATALOG_NAME}"),
            "    path: skills/code-review",
            "    reason: required-by:pack:function.engineering",
            &format!("  {CORE_SKILL}:"),
            &format!("    catalog: {CATALOG_NAME}"),
            "    path: skills/company-context",
            "    reason: required-by:pack:core",
            &format!("  {FRONTEND_SKILL}:"),
            &format!("    catalog: {CATALOG_NAME}"),
            "    path: skills/design-tokens",
            "    reason: required-by:pack:function.engineering.frontend",
            "version: 1",
            "",
        ]
        .join("\n")
    );
}

#[test]
fn is_byte_identical_on_a_second_install_so_nothing_in_it_is_a_timestamp() {
    let project = project();

    project.cli(&["install"]);
    let first = read_lock(&project);

    project.cli(&["install"]);

    assert_eq!(read_lock(&project), first);
}

#[test]
fn carries_no_absolute_path_so_a_team_can_commit_one_file_between_them() {
    let project = project();

    project.cli(&["install"]);

    assert!(!read_lock(&project).contains(&*project.root.to_string_lossy()));
}

#[test]
fn keeps_every_section_even_when_a_project_selects_nothing() {
    let project = project();

    project.write_profile(&[], None, &[]);
    project.cli(&["install"]);

    // An emptied section reads as `{}` rather than vanishing: losing the last MCP server should
    // show up in the diff as a change to `mcps`, not as a key a reader has to notice the absence
    // of.
    assert_eq!(
        read_lock(&project),
        [
            "catalogs:",
            &format!("  {CATALOG_NAME}:"),
            &format!("    source: {CATALOG_SOURCE}"),
            "hooks: {}",
            "mcps: {}",
            "packs: {}",
            "skills: {}",
            "version: 1",
            "",
        ]
        .join("\n")
    );
}

#[test]
fn records_the_reason_each_item_was_selected_in_explains_form() {
    let project = project();

    project.write_profile(
        &["project.acme"],
        None,
        &[&format!(
            "  - {{ skill: \"{CATALOG_NAME}/{ENGINEERING_SKILL}\" }}"
        )],
    );
    project.cli(&["install"]);

    let lock = parse_yaml_mapping(&read_lock(&project), LOCK_FILENAME).expect("a lock");
    let skills = lock.require_mapping("skills").unwrap();
    let reason_of = |section: &crate::model::yaml::YamlMapping, name: &str| {
        section
            .require_mapping(name)
            .unwrap()
            .require_string("reason")
            .unwrap()
    };

    assert_eq!(
        reason_of(&skills, ENGINEERING_SKILL),
        format!("skill:{CATALOG_NAME}/{ENGINEERING_SKILL}")
    );
    assert_eq!(
        reason_of(&skills, PROJECT_SKILL),
        "required-by:pack:project.acme"
    );
    assert_eq!(
        reason_of(&lock.require_mapping("packs").unwrap(), "project.acme"),
        format!("pack:{CATALOG_NAME}/project.acme")
    );
    assert_eq!(
        reason_of(&skills, CORE_SKILL),
        format!("required-by:skill:{PROJECT_SKILL}")
    );
    assert_eq!(
        reason_of(&lock.require_mapping("mcps").unwrap(), "fixture"),
        format!("required-by:skill:{PROJECT_SKILL}")
    );
}

#[test]
fn records_a_command_line_hook_as_config_values_with_no_bytes_to_pin() {
    let project = project();

    write_catalog_hook(
        &project,
        "notify",
        &["event: Stop", "type: command", "command: ./notify"],
        None,
    );
    project.write_profile(
        &[],
        None,
        &[&format!("  - {{ hook: \"{CATALOG_NAME}/notify\" }}")],
    );
    project.cli(&["install"]);

    let entry = parse_yaml_mapping(&read_lock(&project), LOCK_FILENAME)
        .unwrap()
        .require_mapping("hooks")
        .unwrap()
        .require_mapping("notify")
        .unwrap();

    // A hook whose `command` is a command line ships no bytes, so there is nothing to pin: it
    // takes `LockMcp`'s shape, and `catalog` is all a reader needs to find the document.
    assert_eq!(entry.keys(), ["catalog", "reason"]);
    assert_eq!(entry.require_string("catalog").unwrap(), CATALOG_NAME);
    assert_eq!(
        entry.require_string("reason").unwrap(),
        format!("hook:{CATALOG_NAME}/notify")
    );
}

#[test]
fn pins_where_a_hooks_bytes_came_from_only_when_it_ships_a_script() {
    let project = project();

    write_catalog_hook(
        &project,
        "block-rm",
        &["event: PreToolUse", "type: script", "command: hook.sh"],
        Some(("hook.sh", "#!/bin/sh\nexit 0\n")),
    );
    write_catalog_hook(
        &project,
        "announce",
        &[
            "event: Stop",
            "type: command",
            "command: npx --yes say done",
        ],
        None,
    );
    project.write_profile(
        &[],
        None,
        &[
            &format!("  - {{ hook: \"{CATALOG_NAME}/block-rm\" }}"),
            &format!("  - {{ hook: \"{CATALOG_NAME}/announce\" }}"),
        ],
    );

    // Through `build_lock` rather than the CLI, so the commit is a value rather than something a
    // git source has to supply.
    let catalog = catalog_at(&project, "abc1234");
    let lock = build_lock(
        std::slice::from_ref(&catalog),
        &bundle_from(&project, std::slice::from_ref(&catalog)),
    )
    .expect("a lock");
    let hooks = parse_yaml_mapping(&serialize_lock(&lock), LOCK_FILENAME)
        .unwrap()
        .require_mapping("hooks")
        .unwrap();

    // `command: hook.sh` names a file the hook's directory holds, so an install materializes
    // those bytes and the lock says which they were. `path` is the catalog-relative directory, as
    // a skill's is.
    let shipping = hooks.require_mapping("block-rm").unwrap();

    assert_eq!(shipping.keys(), ["catalog", "commit", "path", "reason"]);
    assert_eq!(shipping.require_string("catalog").unwrap(), CATALOG_NAME);
    assert_eq!(shipping.require_string("path").unwrap(), "hooks/block-rm");
    assert_eq!(shipping.require_string("commit").unwrap(), "abc1234");
    assert_eq!(
        shipping.require_string("reason").unwrap(),
        format!("hook:{CATALOG_NAME}/block-rm")
    );

    // `npx --yes say done` is a command line, so the same catalog entry ships nothing and pins
    // nothing.
    let inert = hooks.require_mapping("announce").unwrap();

    assert_eq!(inert.keys(), ["catalog", "reason"]);
    assert_eq!(inert.require_string("catalog").unwrap(), CATALOG_NAME);
}

#[test]
fn quotes_a_commit_and_a_ref_a_yaml_parser_would_otherwise_read_as_numbers() {
    // `1234567` unquoted parses as an integer and `1e5` as a float, so an unquoted lock would pin
    // a different commit than the one installed.
    let project = project();
    let catalog = Catalog {
        r#ref: Some("1e5".to_owned()),
        ..catalog_at(&project, "1234567")
    };
    let text = serialize_lock(
        &build_lock(
            std::slice::from_ref(&catalog),
            &bundle_from(&project, std::slice::from_ref(&catalog)),
        )
        .expect("a lock"),
    );

    assert!(text.contains("    commit: \"1234567\"\n"), "{text}");
    assert!(text.contains("    ref: \"1e5\"\n"), "{text}");

    let lock = parse_yaml_mapping(&text, LOCK_FILENAME).unwrap();
    let entry = lock
        .require_mapping("catalogs")
        .unwrap()
        .require_mapping(CATALOG_NAME)
        .unwrap();

    assert_eq!(entry.require_string("commit").unwrap(), "1234567");
    assert_eq!(entry.require_string("ref").unwrap(), "1e5");
    // Every catalog skill inherits it, so the same quoting has to hold there too.
    assert_eq!(
        lock.require_mapping("skills")
            .unwrap()
            .require_mapping(CORE_SKILL)
            .unwrap()
            .require_string("commit")
            .unwrap(),
        "1234567"
    );
}

// serialize_lock, without resolving anything

#[test]
fn emits_every_section_and_quotes_what_would_read_as_a_number() {
    let mut lock = Lock {
        version: LOCK_VERSION,
        catalogs: IndexMap::new(),
        packs: IndexMap::new(),
        skills: IndexMap::new(),
        mcps: IndexMap::new(),
        hooks: IndexMap::new(),
    };

    lock.catalogs.insert(
        CATALOG_NAME.to_owned(),
        LockCatalog {
            source: CATALOG_SOURCE.to_owned(),
            r#ref: Some("1e5".to_owned()),
            commit: Some("1234567".to_owned()),
        },
    );
    lock.hooks.insert(
        "notify".to_owned(),
        LockHook {
            catalog: CATALOG_NAME.to_owned(),
            path: None,
            commit: None,
            reason: format!("hook:{CATALOG_NAME}/notify"),
        },
    );

    assert_eq!(
        serialize_lock(&lock),
        [
            "catalogs:",
            "  company:",
            "    commit: \"1234567\"",
            "    ref: \"1e5\"",
            "    source: path:../catalog",
            "hooks:",
            "  notify:",
            "    catalog: company",
            "    reason: hook:company/notify",
            "mcps: {}",
            "packs: {}",
            "skills: {}",
            "version: 1",
            "",
        ]
        .join("\n")
    );
}

// ambit install --frozen

#[test]
fn succeeds_when_the_lock_on_disk_is_what_resolution_produces() {
    let project = project();

    project.cli(&["install"]);
    let before = read_lock(&project);

    let result = project.cli(&["install", "--frozen"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(read_lock(&project), before);
}

#[test]
fn exits_5_when_the_project_has_no_lock_and_writes_nothing() {
    let project = project();
    let result = project.cli(&["install", "--frozen"]);

    assert_eq!(result.code, ExitCode::Drift);
    assert!(
        result
            .stderr
            .contains(&format!("{LOCK_FILENAME} is out of date"))
    );
    assert!(result.stderr.contains(&format!("has no {LOCK_FILENAME}")));
    assert!(!lock_exists(&project));
    assert_eq!(project.installed_skills(), Vec::<String>::new());
}

#[test]
fn exits_5_when_resolution_would_change_the_lock_leaving_the_project_as_it_was() {
    let project = project();

    project.cli(&["install"]);
    let before = read_lock(&project);
    let installed_before = project.installed_skills();

    project.write_profile(&["project.acme"], None, &[]);
    let result = project.cli(&["install", "--frozen"]);

    assert_eq!(result.code, ExitCode::Drift);
    assert!(
        result
            .stderr
            .contains("produces a different ambit.lock than the one on disk")
    );
    assert_eq!(read_lock(&project), before);
    assert_eq!(project.installed_skills(), installed_before);
}

#[test]
fn exits_5_for_a_lock_that_says_the_same_thing_in_different_bytes() {
    let project = project();

    project.cli(&["install"]);
    // Reformatting is drift too: `--frozen` is asked whether install would rewrite the file, and
    // a lock ambit did not emit is one ambit would rewrite.
    project.write(LOCK_FILENAME, &read_lock(&project).replace('\n', "\n\n"));

    assert_eq!(project.cli(&["install", "--frozen"]).code, ExitCode::Drift);
}

// assert_lock_current, against a lock on disk

#[test]
fn refuses_a_missing_lock_naming_the_project() {
    let project = Project::new();
    let error = assert_lock_current(&project.dir, "version: 1\n").expect_err("a refusal");

    assert_eq!(error.code, ExitCode::Drift);
    assert_eq!(
        error.format(),
        [
            "error: ambit.lock is out of date".to_owned(),
            format!(
                "       `--frozen` compares against a committed lock, and {} has no ambit.lock",
                crate::util::path::to_slash(&project.dir)
            ),
            "       run `ambit install` without `--frozen`, then commit the result".to_owned(),
        ]
        .join("\n")
    );
}
