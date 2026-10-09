use pretty_assertions::assert_eq;

use super::*;
use crate::errors::ExitCode;
use crate::model::catalog::{CatalogParseOptions, merge_catalogs, parse_catalog_directory};
use crate::model::config::load_project_config;
use crate::model::lock_file::LockedItem;
use crate::model::yaml::parse_yaml_mapping;
use crate::project::install::fixture::*;
use crate::resolution::resolve::resolve_bundle;

const CATALOG_SOURCE: &str = "path:../catalog";

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

fn write_catalog_hook(project: &Project, name: &str, body: &[&str], script: Option<(&str, &str)>) {
    let mut lines = vec![format!("name: {name}")];

    lines.extend(body.iter().map(|&line| line.to_owned()));
    lines.push(String::new());
    project.write_catalog(&format!("hooks/{name}/hook.yml"), &lines.join("\n"));

    if let Some((file, contents)) = script {
        project.write_catalog(&format!("hooks/{name}/{file}"), contents);
    }
}

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

fn bundle_from(project: &Project, catalogs: &[Catalog]) -> Bundle {
    let config = load_project_config(&project.dir).expect("a valid config");

    resolve_bundle(&config, &merge_catalogs(catalogs)).expect("a resolvable profile")
}

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
            "hooks:",
            "  guard-secrets:",
            &format!("    catalog: {CATALOG_NAME}"),
            "    exec: sha256-4490456a18e1ceffc2803db69ac395625d5fdcdef0d8d651a981ecfceecd39ec",
            "    path: hooks/guard-secrets",
            "    reason: required-by:pack:function.engineering",
            "  session-notes:",
            &format!("    catalog: {CATALOG_NAME}"),
            "    exec: sha256-3e2df317065141784006837c665b3b1c1d3cba2ea2b2eb3bfeb13bb243d17a3b",
            "    reason: required-by:pack:core",
            "mcps:",
            "  linter:",
            &format!("    catalog: {CATALOG_NAME}"),
            "    exec: sha256-7ed71c619f682cbad2712535a3a86dd71f00b953c1d918cee3d86b8c5e6752c7",
            "    reason: required-by:pack:function.engineering",
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

    assert_eq!(entry.keys(), ["catalog", "exec", "reason"]);
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

    let catalog = catalog_at(&project, "abc1234");
    let digests = ItemDigests {
        hooks: IndexMap::from([
            ("block-rm".to_owned(), "sha256-blockrm".to_owned()),
            ("announce".to_owned(), "sha256-ignored".to_owned()),
        ]),
        ..ItemDigests::default()
    };
    let lock = build_lock(
        std::slice::from_ref(&catalog),
        &bundle_from(&project, std::slice::from_ref(&catalog)),
        &digests,
    )
    .expect("a lock");
    let hooks = parse_yaml_mapping(&serialize_lock(&lock), LOCK_FILENAME)
        .unwrap()
        .require_mapping("hooks")
        .unwrap();

    let shipping = hooks.require_mapping("block-rm").unwrap();

    assert_eq!(
        shipping.keys(),
        ["catalog", "commit", "digest", "exec", "path", "reason"]
    );
    assert_eq!(shipping.require_string("catalog").unwrap(), CATALOG_NAME);
    assert_eq!(shipping.require_string("path").unwrap(), "hooks/block-rm");
    assert_eq!(shipping.require_string("commit").unwrap(), "abc1234");
    assert_eq!(shipping.require_string("digest").unwrap(), "sha256-blockrm");
    assert_eq!(
        shipping.require_string("reason").unwrap(),
        format!("hook:{CATALOG_NAME}/block-rm")
    );

    let inert = hooks.require_mapping("announce").unwrap();

    assert_eq!(inert.keys(), ["catalog", "exec", "reason"]);
    assert_eq!(inert.require_string("catalog").unwrap(), CATALOG_NAME);
}

#[test]
fn quotes_a_commit_and_a_ref_a_yaml_parser_would_otherwise_read_as_numbers() {
    let project = project();
    let catalog = Catalog {
        r#ref: Some("1e5".to_owned()),
        ..catalog_at(&project, "1234567")
    };
    let text = serialize_lock(
        &build_lock(
            std::slice::from_ref(&catalog),
            &bundle_from(&project, std::slice::from_ref(&catalog)),
            &ItemDigests::default(),
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
            digest: None,
            exec: "sha256-exec".to_owned(),
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
            "    exec: sha256-exec",
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
    project.write(LOCK_FILENAME, &read_lock(&project).replace('\n', "\n\n"));

    assert_eq!(project.cli(&["install", "--frozen"]).code, ExitCode::Drift);
}

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

fn pinned_lock(commit: &str, digest: &str) -> Lock {
    let mut lock = Lock {
        version: LOCK_VERSION,
        catalogs: IndexMap::new(),
        packs: IndexMap::new(),
        skills: IndexMap::new(),
        mcps: IndexMap::new(),
        hooks: IndexMap::new(),
    };

    lock.skills.insert(
        CORE_SKILL.to_owned(),
        LockSkill {
            catalog: CATALOG_NAME.to_owned(),
            path: format!("skills/{CORE_SKILL}"),
            commit: Some(commit.to_owned()),
            digest: Some(digest.to_owned()),
            reason: format!("skill:{CATALOG_NAME}/{CORE_SKILL}"),
        },
    );
    lock
}

fn read_back(lock: &Lock) -> LockedItems {
    let project = Project::new();

    write_lock_text(&project.dir, &serialize_lock(lock)).unwrap();
    read_locked_items(&project.dir)
        .unwrap()
        .expect("a lock to read")
}

#[test]
fn reads_back_every_item_field_and_tolerates_a_missing_digest() {
    let mut lock = pinned_lock("abc1234", "sha256-one");
    let earlier = read_back(&lock);

    assert_eq!(
        earlier.skills[CORE_SKILL],
        LockedItem {
            catalog: Some(CATALOG_NAME.to_owned()),
            path: Some(format!("skills/{CORE_SKILL}")),
            commit: Some("abc1234".to_owned()),
            digest: Some("sha256-one".to_owned()),
            exec: None,
        }
    );

    lock.skills[CORE_SKILL].digest = None;

    assert_eq!(read_back(&lock).skills[CORE_SKILL].digest, None);
    assert_eq!(read_locked_items(&Project::new().dir).unwrap(), None);
}

#[test]
fn refuses_a_tree_whose_digest_moved_under_the_same_commit() {
    let earlier = read_back(&pinned_lock("abc1234", "sha256-one"));
    let error = verify_digests(&earlier, &pinned_lock("abc1234", "sha256-two")).unwrap_err();

    assert_eq!(error.code, ExitCode::Drift);
    assert_eq!(
        error.message,
        format!("skill \"{CORE_SKILL}\" does not match the digest {LOCK_FILENAME} records")
    );
    assert_eq!(
        error.detail[..2],
        [
            format!("{LOCK_FILENAME} records sha256-one for skills/{CORE_SKILL} at commit abc1234"),
            "the catalog checkout holds sha256-two".to_owned(),
        ]
    );
}

#[test]
fn accepts_a_new_commit_a_matching_digest_and_an_entry_with_none_recorded() {
    let earlier = read_back(&pinned_lock("abc1234", "sha256-one"));

    verify_digests(&earlier, &pinned_lock("def5678", "sha256-two")).unwrap();
    verify_digests(&earlier, &pinned_lock("abc1234", "sha256-one")).unwrap();

    let mut undigested = pinned_lock("abc1234", "sha256-one");

    undigested.skills[CORE_SKILL].digest = None;

    let earlier = read_back(&undigested);

    verify_digests(&earlier, &pinned_lock("abc1234", "sha256-two")).unwrap();
}
