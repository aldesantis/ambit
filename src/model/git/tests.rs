use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;
use crate::errors::ExitCode;
use crate::model::catalog::{CatalogLoadOptions, load_catalogs};
use crate::model::config::load_project_config;
use crate::model::lock_file::LOCK_FILENAME;
use crate::model::sources::SourceContext;
use crate::model::state::{ArtifactKind, ArtifactMode, STATE_DIRNAME, STATE_FILENAME, parse_state};
use crate::model::yaml::{YamlMapping, parse_yaml_mapping};
use crate::test_support::fixture_catalog::{
    FixtureGitCatalog, build_fixture_catalog, build_fixture_git_catalog,
    commit_fixture_git_revision, file_url,
};
use crate::test_support::{CliResult, run_cli, tempdir, test_env};

const CATALOG_NAME: &str = "company";
const CORE_SKILL: &str = "company-context";
const SKILLS_DIR: &str = ".agents/skills";

const TAGS: &[&str] = &["core", "function.engineering", "function.engineering.*"];

const PER_SOURCE_FILES: &[&str] = &["ambit.yml", "ambit.lock"];

struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    cache_dir: PathBuf,
    env: Env,
    fixture: FixtureGitCatalog,
    git_project: PathBuf,
    path_project: PathBuf,
}

fn world() -> World {
    let dir = tempdir();
    let root = dir.path().to_path_buf();
    let env = test_env(&root);
    let cache_dir = root.join("cache");

    build_fixture_catalog(&root.join("catalog")).expect("build the fixture catalog");
    let fixture = build_fixture_git_catalog(&root.join("remote")).expect("build the repository");

    let git_project = root.join("from-git");
    let path_project = root.join("from-path");
    write_project(&git_project, &fixture.url, None, &[]);
    write_project(&path_project, "path:../catalog", None, &[]);

    World {
        _dir: dir,
        root,
        cache_dir,
        env,
        fixture,
        git_project,
        path_project,
    }
}

fn write_project(dir: &Path, source: &str, r#ref: Option<&str>, extra: &[&str]) {
    let ref_line = r#ref.map_or(String::new(), |r#ref| format!("    ref: \"{ref}\"\n"));
    let requires: Vec<String> = TAGS
        .iter()
        .map(|tag| format!("  - {{ pack: \"{CATALOG_NAME}/{tag}\" }}"))
        .collect();
    let extra: String = extra.iter().flat_map(|line| [*line, "\n"]).collect();

    std::fs::create_dir_all(dir).expect("create the project");
    std::fs::write(
        dir.join("ambit.yml"),
        format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: {source}\n    trust: full\n{ref_line}requires:\n{}\n{extra}",
            requires.join("\n")
        ),
    )
    .expect("write ambit.yml");
}

impl World {
    fn cli(&self, dir: &Path, argv: &[&str]) -> CliResult {
        let dir = dir.to_string_lossy();
        let mut args = argv.to_vec();
        args.extend(["--project", &dir]);
        run_cli(&args, &self.root, &self.env)
    }

    fn cache_paths(&self) -> (PathBuf, PathBuf) {
        let cache = cache_root(&self.env);
        let key = git_cache_key(&self.fixture.url);

        (
            join(&join(&cache, REPOS_DIRNAME), &format!("{key}.git")),
            join(&join(&cache, SOURCES_DIRNAME), &key),
        )
    }

    fn git_catalog(&self) -> (PathBuf, Option<String>) {
        let config = load_project_config(&self.git_project).expect("load the config");
        let context = SourceContext {
            project_dir: self.git_project.clone(),
            env: self.env.clone(),
            offline: false,
        };
        let catalogs = load_catalogs(&config, &context, &mut CatalogLoadOptions::default())
            .expect("load the catalogs");
        let catalog = catalogs
            .into_iter()
            .next()
            .expect("expected the project to declare one catalog");

        (catalog.root, catalog.commit)
    }

    fn request(&self, r#ref: Option<&str>) -> GitFetchRequest {
        GitFetchRequest {
            url: self.fixture.url.clone(),
            r#ref: r#ref.map(str::to_owned),
            subject: format!("catalog \"{CATALOG_NAME}\""),
            r#where: "(ambit.yml line 3)".to_owned(),
            env: self.env.clone(),
            cwd: self.root.clone(),
            ..GitFetchRequest::default()
        }
    }
}

fn installed(dir: &Path) -> BTreeMap<String, String> {
    fn walk(current: &Path, relative: &str, found: &mut BTreeMap<String, String>) {
        for entry in crate::util::fs::read_dir_names(current).expect("list a directory") {
            let within = if relative.is_empty() {
                entry.clone()
            } else {
                format!("{relative}/{entry}")
            };

            if PER_SOURCE_FILES.contains(&within.as_str()) {
                continue;
            }

            let absolute = current.join(&entry);

            if absolute.is_dir() {
                walk(&absolute, &within, found);
            } else {
                found.insert(
                    within,
                    crate::util::fs::read_text(&absolute).expect("read a file"),
                );
            }
        }
    }

    let mut found = BTreeMap::new();
    walk(dir, "", &mut found);
    found
}

fn assert_success(result: &CliResult) {
    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
}

fn remove(target: &Path) {
    crate::util::fs::rm_rf(target).expect("remove");
}

#[test]
fn installs_exactly_what_the_same_catalog_installs_from_a_directory() {
    let w = world();
    assert_success(&w.cli(&w.path_project, &["install", "--copy"]));
    assert_success(&w.cli(&w.git_project, &["install"]));

    assert_eq!(installed(&w.git_project), installed(&w.path_project));
    assert!(installed(&w.git_project).contains_key(&format!("{SKILLS_DIR}/{CORE_SKILL}/SKILL.md")));
}

#[test]
fn copies_its_skills_since_a_commit_is_not_a_working_tree_anyone_edits() {
    let w = world();
    assert_success(&w.cli(&w.git_project, &["install"]));

    let target = w.git_project.join(SKILLS_DIR).join(CORE_SKILL);

    assert!(!target.is_symlink());
    let state = crate::util::fs::read_text(&w.git_project.join(STATE_DIRNAME).join(STATE_FILENAME))
        .expect("read state");
    let artifacts = parse_state(&state, STATE_FILENAME)
        .expect("parse state")
        .artifacts;

    assert!(artifacts.iter().any(|artifact| {
        artifact.path == format!("{SKILLS_DIR}/{CORE_SKILL}")
            && artifact.kind == ArtifactKind::SkillDir
            && artifact.mode == Some(ArtifactMode::Copy)
    }));
}

#[test]
fn clones_into_the_cache_keyed_by_host_and_path_and_checks_the_commit_out_there() {
    let w = world();
    assert_success(&w.cli(&w.git_project, &["install"]));

    let (clone, checkouts) = w.cache_paths();

    assert!(clone.join("HEAD").exists());
    assert!(
        checkouts
            .join(&w.fixture.commit)
            .join("skills/company-context/SKILL.md")
            .exists()
    );
    assert!(clone.starts_with(w.cache_dir.join("ambit")));
}

#[test]
fn reports_the_commit_the_catalog_is_pinned_to() {
    let w = world();
    let (root, commit) = w.git_catalog();

    assert_eq!(commit.as_deref(), Some(w.fixture.commit.as_str()));
    assert_eq!(root, w.cache_paths().1.join(&w.fixture.commit));
}

#[test]
fn resolves_every_spelling_of_the_ref_to_the_same_commit() {
    let w = world();
    let abbreviated = &w.fixture.commit[..8];

    for asked in [
        None,
        Some("main"),
        Some("v1"),
        Some(w.fixture.commit.as_str()),
        Some(abbreviated),
    ] {
        write_project(&w.git_project, &w.fixture.url, asked, &[]);

        assert_eq!(
            w.git_catalog().1.as_deref(),
            Some(w.fixture.commit.as_str()),
            "{asked:?}"
        );
    }
}

#[test]
fn resolves_from_the_cache_on_a_second_run_with_the_remote_gone() {
    let w = world();
    assert_success(&w.cli(&w.git_project, &["install"]));
    let before = installed(&w.git_project);

    remove(&w.fixture.repo);

    assert_success(&w.cli(&w.git_project, &["install"]));
    assert_eq!(installed(&w.git_project), before);
}

#[test]
fn checks_a_commit_out_once_and_reuses_the_checkout() {
    let w = world();
    w.cli(&w.git_project, &["install"]);
    w.cli(&w.git_project, &["install"]);

    let mut names = crate::util::fs::read_dir_names(&w.cache_paths().1).expect("list checkouts");
    names.sort();

    assert_eq!(
        names,
        [
            w.fixture.commit.clone(),
            format!("{}.ready", w.fixture.commit)
        ]
    );
}

#[test]
fn shares_one_clone_between_the_url_and_its_git_spelling() {
    let w = world();
    w.cli(&w.git_project, &["install"]);
    write_project(&w.git_project, &format!("git:{}", w.fixture.url), None, &[]);
    remove(&w.fixture.repo);

    assert_success(&w.cli(&w.git_project, &["install"]));
    assert_eq!(
        crate::util::fs::read_dir_names(&cache_root(&w.env).join(REPOS_DIRNAME))
            .expect("list repos"),
        ["local"]
    );
}

fn lock(dir: &Path) -> YamlMapping {
    let text = crate::util::fs::read_text(&dir.join(LOCK_FILENAME)).expect("read the lock");

    parse_yaml_mapping(&text, LOCK_FILENAME).expect("parse the lock")
}

#[test]
fn pins_the_catalog_to_the_commit_its_ref_resolved_to_keeping_the_ref_it_was_asked_for() {
    let w = world();
    write_project(&w.git_project, &w.fixture.url, Some(&w.fixture.tag), &[]);
    assert_success(&w.cli(&w.git_project, &["install"]));

    let entry = lock(&w.git_project)
        .require_mapping("catalogs")
        .and_then(|catalogs| catalogs.require_mapping(CATALOG_NAME))
        .expect("a catalog entry");

    assert_eq!(entry.require_string("source").unwrap(), w.fixture.url);
    assert_eq!(entry.require_string("ref").unwrap(), w.fixture.tag);
    assert_eq!(entry.require_string("commit").unwrap(), w.fixture.commit);
}

#[test]
fn pins_every_skill_it_installed_to_that_same_commit() {
    let w = world();
    w.cli(&w.git_project, &["install"]);

    let entry = lock(&w.git_project)
        .require_mapping("skills")
        .and_then(|skills| skills.require_mapping(CORE_SKILL))
        .expect("a skill entry");

    assert_eq!(entry.require_string("catalog").unwrap(), CATALOG_NAME);
    assert_eq!(entry.require_string("commit").unwrap(), w.fixture.commit);
}

#[test]
fn leaves_the_commit_out_for_a_catalog_read_from_a_directory() {
    let w = world();
    w.cli(&w.path_project, &["install"]);

    let entry = lock(&w.path_project)
        .require_mapping("catalogs")
        .and_then(|catalogs| catalogs.require_mapping(CATALOG_NAME))
        .expect("a catalog entry");

    assert_eq!(entry.optional_string("commit").unwrap(), None);
    assert_eq!(entry.require_string("source").unwrap(), "path:../catalog");
}

#[test]
fn exits_2_for_a_ref_the_repository_does_not_have() {
    let w = world();
    write_project(&w.git_project, &w.fixture.url, Some("nope"), &[]);

    let result = w.cli(&w.git_project, &["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains(&format!(
        "cannot resolve ref \"nope\" for catalog \"{CATALOG_NAME}\""
    )));
    assert!(result.stderr.contains("omit it to take the default branch"));
    assert!(!w.git_project.join(SKILLS_DIR).exists());
}

#[test]
fn exits_2_for_a_ref_git_would_read_as_an_option() {
    let w = world();
    write_project(
        &w.git_project,
        &w.fixture.url,
        Some("--upload-pack=touch"),
        &[],
    );

    let result = w.cli(&w.git_project, &["install"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains(&format!("catalog \"{CATALOG_NAME}\" has an unusable ref"))
    );
}

#[test]
fn exits_4_for_a_repository_that_is_not_there() {
    let w = world();
    write_project(
        &w.git_project,
        &file_url(&w.root.join("missing.git")),
        None,
        &[],
    );

    let result = w.cli(&w.git_project, &["install"]);

    assert_eq!(result.code, ExitCode::Network);
    assert!(
        result
            .stderr
            .contains(&format!("cannot clone catalog \"{CATALOG_NAME}\""))
    );
    assert!(result.stderr.contains("git said:"));
}

fn warm_the_cache(w: &World) {
    assert_success(&w.cli(&w.git_project, &["install"]));
}

#[test]
fn offline_resolves_entirely_from_the_cache_with_the_remote_gone() {
    let w = world();
    warm_the_cache(&w);
    remove(&w.fixture.repo);

    let result = w.cli(&w.git_project, &["resolve", "--offline"]);

    assert_success(&result);
    assert!(result.stdout.contains(CORE_SKILL));
}

#[test]
fn offline_installs_a_project_that_never_fetched_from_the_cache_another_project_filled() {
    let w = world();
    warm_the_cache(&w);
    let second = w.root.join("from-cache");

    write_project(&second, &w.fixture.url, None, &[]);
    remove(&w.fixture.repo);

    assert_success(&w.cli(&second, &["install", "--offline"]));
    assert!(installed(&second).contains_key(&format!("{SKILLS_DIR}/{CORE_SKILL}/SKILL.md")));
}

#[test]
fn offline_checks_a_commit_out_from_a_clone_it_already_has() {
    let w = world();
    warm_the_cache(&w);
    let (_, checkouts) = w.cache_paths();

    remove(&checkouts.join(&w.fixture.commit));
    remove(&checkouts.join(format!("{}.ready", w.fixture.commit)));
    remove(&w.fixture.repo);

    assert_success(&w.cli(&w.git_project, &["install", "--offline"]));
    assert!(
        checkouts
            .join(&w.fixture.commit)
            .join("skills/company-context/SKILL.md")
            .exists()
    );
}

#[test]
fn offline_exits_4_naming_the_catalog_it_would_have_had_to_clone_and_clones_nothing() {
    let w = world();
    let result = w.cli(&w.git_project, &["install", "--offline"]);

    assert_eq!(result.code, ExitCode::Network);
    assert!(
        result
            .stderr
            .contains(&format!("catalog \"{CATALOG_NAME}\" is not in the cache"))
    );
    assert!(result.stderr.contains(&w.fixture.url));
    assert!(result.stderr.contains("without `--offline`"));
    assert!(!w.cache_paths().0.exists());
    assert!(!w.git_project.join(SKILLS_DIR).exists());
}

#[test]
fn offline_exits_4_for_a_ref_the_cached_clone_was_never_told_about_without_fetching() {
    let w = world();
    warm_the_cache(&w);
    write_project(&w.git_project, &w.fixture.url, Some("v2"), &[]);
    remove(&w.fixture.repo);

    let result = w.cli(&w.git_project, &["install", "--offline"]);

    assert_eq!(result.code, ExitCode::Network);
    assert!(result.stderr.contains(&format!(
        "cannot resolve ref \"v2\" from the cache for catalog \"{CATALOG_NAME}\""
    )));
    assert!(!result.stderr.contains("cannot fetch"));
}

#[test]
fn offline_has_nothing_to_say_about_a_catalog_read_from_a_directory() {
    let w = world();

    assert_success(&w.cli(&w.path_project, &["install", "--offline"]));
    assert!(
        installed(&w.path_project).contains_key(&format!("{SKILLS_DIR}/{CORE_SKILL}/SKILL.md"))
    );
}

#[test]
fn fetch_clones_into_the_cache_and_checks_the_commit_out_there() {
    let w = world();
    let fetched = fetch_git_source(&w.request(None)).expect("fetch");
    let (clone, checkouts) = w.cache_paths();

    assert_eq!(fetched.commit, w.fixture.commit);
    assert_eq!(fetched.root, checkouts.join(&w.fixture.commit));
    assert_eq!(fetched.moving, None);
    assert!(clone.join("HEAD").exists());
    assert!(clone.starts_with(w.cache_dir.join("ambit")));
    assert!(
        fetched
            .root
            .join("skills/company-context/SKILL.md")
            .exists()
    );

    let mut names = crate::util::fs::read_dir_names(&checkouts).expect("list checkouts");
    names.sort();

    assert_eq!(
        names,
        [
            w.fixture.commit.clone(),
            format!("{}.ready", w.fixture.commit)
        ]
    );
}

#[test]
fn fetch_resolves_every_spelling_of_the_ref_to_the_same_commit() {
    let w = world();
    let abbreviated = w.fixture.commit[..8].to_owned();

    for asked in [
        None,
        Some("main"),
        Some("v1"),
        Some(w.fixture.commit.as_str()),
        Some(abbreviated.as_str()),
    ] {
        let fetched = fetch_git_source(&w.request(asked)).expect("fetch");

        assert_eq!(fetched.commit, w.fixture.commit, "{asked:?}");
    }
}

#[test]
fn fetch_answers_from_the_cache_with_the_remote_gone() {
    let w = world();
    fetch_git_source(&w.request(None)).expect("fetch");
    remove(&w.fixture.repo);

    let offline = GitFetchRequest {
        offline: true,
        ..w.request(Some("v1"))
    };

    assert_eq!(
        fetch_git_source(&offline).expect("fetch offline").commit,
        w.fixture.commit
    );
    assert_eq!(
        fetch_git_source(&w.request(None)).expect("fetch").commit,
        w.fixture.commit
    );
}

#[test]
fn fetch_shares_one_clone_between_the_url_and_a_trailing_slash_spelling() {
    let w = world();
    fetch_git_source(&w.request(None)).expect("fetch");
    remove(&w.fixture.repo);

    let respelled = GitFetchRequest {
        url: format!("{}/", w.fixture.url),
        ..w.request(None)
    };

    fetch_git_source(&respelled).expect("fetch from the cache");
    assert_eq!(
        crate::util::fs::read_dir_names(&cache_root(&w.env).join(REPOS_DIRNAME))
            .expect("list repos"),
        ["local"]
    );
}

#[test]
fn fetch_exits_2_for_a_ref_the_repository_does_not_have() {
    let w = world();
    let error = fetch_git_source(&w.request(Some("nope"))).expect_err("an unknown ref");

    assert_eq!(error.code, ExitCode::Config);
    assert_eq!(
        error.message,
        "cannot resolve ref \"nope\" for catalog \"company\" (ambit.yml line 3)"
    );
    assert!(error.detail[1].contains("omit it to take the default branch"));
}

#[test]
fn fetch_exits_2_for_a_ref_git_would_read_as_an_option() {
    let w = world();

    for r#ref in ["--upload-pack=touch", "a b", " "] {
        let error = fetch_git_source(&w.request(Some(r#ref))).expect_err("an unusable ref");

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(
            error.message,
            "catalog \"company\" has an unusable ref (ambit.yml line 3)"
        );
    }

    assert!(!w.cache_paths().0.exists());
}

#[test]
fn fetch_exits_2_for_a_pin_that_is_not_a_full_commit_sha() {
    let w = world();
    let request = GitFetchRequest {
        pin: Some("main".to_owned()),
        ..w.request(None)
    };
    let error = fetch_git_source(&request).expect_err("an unusable pin");

    assert_eq!(error.code, ExitCode::Config);
    assert_eq!(
        error.message,
        "catalog \"company\" has an unusable pin (ambit.yml line 3)"
    );
}

#[test]
fn fetch_exits_4_for_a_repository_that_is_not_there_and_leaves_nothing() {
    let w = world();
    let request = GitFetchRequest {
        url: file_url(&w.root.join("missing.git")),
        ..w.request(None)
    };
    let error = fetch_git_source(&request).expect_err("a missing repository");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(
        error.message,
        "cannot clone catalog \"company\" (ambit.yml line 3)"
    );
    assert!(error.detail[0].starts_with("git said:"), "{error:?}");
    let repos = cache_root(&w.env).join(REPOS_DIRNAME).join("local");
    let leftovers = crate::util::fs::read_dir_names(&repos).unwrap_or_default();

    assert!(
        leftovers.iter().all(|name| !name.starts_with("missing")),
        "{leftovers:?}"
    );
}

#[test]
fn fetch_offline_exits_4_naming_what_it_would_have_had_to_clone() {
    let w = world();
    let request = GitFetchRequest {
        offline: true,
        ..w.request(None)
    };
    let error = fetch_git_source(&request).expect_err("a cold cache");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(
        error.message,
        "catalog \"company\" is not in the cache (ambit.yml line 3)"
    );
    assert!(error.detail[0].contains(&w.fixture.url));
    assert!(error.detail[1].contains("without `--offline`"));
    assert!(!w.cache_paths().0.exists());
}

#[test]
fn fetch_offline_exits_4_for_a_ref_the_cache_does_not_have_without_fetching() {
    let w = world();
    fetch_git_source(&w.request(None)).expect("fetch");
    let request = GitFetchRequest {
        offline: true,
        ..w.request(Some("v2"))
    };
    let error = fetch_git_source(&request).expect_err("an uncached ref");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(
        error.message,
        "cannot resolve ref \"v2\" from the cache for catalog \"company\" (ambit.yml line 3)"
    );
}

#[test]
fn fetch_offline_refuses_a_refresh() {
    let w = world();

    for refresh in [RefreshMode::Probe, RefreshMode::Advance] {
        let request = GitFetchRequest {
            offline: true,
            refresh: Some(refresh),
            ..w.request(None)
        };
        let error = fetch_git_source(&request).expect_err("an offline refresh");

        assert_eq!(error.code, ExitCode::Network);
        assert_eq!(
            error.message,
            "cannot check catalog \"company\" for updates offline (ambit.yml line 3)"
        );
    }
}

#[test]
fn fetch_checks_a_commit_out_offline_from_a_clone_it_already_has() {
    let w = world();
    fetch_git_source(&w.request(None)).expect("fetch");
    let (_, checkouts) = w.cache_paths();

    remove(&checkouts.join(&w.fixture.commit));
    remove(&checkouts.join(format!("{}.ready", w.fixture.commit)));
    remove(&w.fixture.repo);

    let request = GitFetchRequest {
        offline: true,
        ..w.request(None)
    };
    let fetched = fetch_git_source(&request).expect("fetch offline");

    assert!(
        fetched
            .root
            .join("skills/company-context/SKILL.md")
            .exists()
    );
}

fn advance_the_remote(w: &World) -> String {
    commit_fixture_git_revision(
        &w.fixture,
        &[(
            "skills/company-context/SKILL.md",
            Some("---\nname: company-context\ndescription: moved\n---\n"),
        )],
        "move the branch",
    )
    .expect("commit a second revision")
}

#[test]
fn fetch_stays_on_the_cached_commit_until_asked_to_advance() {
    let w = world();
    fetch_git_source(&w.request(Some("main"))).expect("fetch");
    let moved = advance_the_remote(&w);

    assert_eq!(
        fetch_git_source(&w.request(Some("main")))
            .expect("fetch")
            .commit,
        w.fixture.commit
    );

    let probe = GitFetchRequest {
        refresh: Some(RefreshMode::Probe),
        ..w.request(Some("main"))
    };
    let probed = fetch_git_source(&probe).expect("probe");

    assert_eq!(probed.commit, moved);
    assert_eq!(probed.moving, Some(true));
    assert_eq!(
        fetch_git_source(&w.request(Some("main")))
            .expect("fetch")
            .commit,
        w.fixture.commit
    );

    let advance = GitFetchRequest {
        refresh: Some(RefreshMode::Advance),
        ..w.request(Some("main"))
    };
    let advanced = fetch_git_source(&advance).expect("advance");

    assert_eq!(advanced.commit, moved);
    assert_eq!(advanced.moving, Some(true));
    assert_eq!(
        fetch_git_source(&w.request(Some("main")))
            .expect("fetch")
            .commit,
        moved
    );
}

#[test]
fn fetch_reports_a_commit_ref_as_standing_and_a_tag_as_moving() {
    let w = world();

    for (asked, moving) in [
        (Some(w.fixture.commit.as_str()), false),
        (Some("v1"), true),
        (None, true),
    ] {
        for refresh in [RefreshMode::Probe, RefreshMode::Advance] {
            let request = GitFetchRequest {
                refresh: Some(refresh),
                ..w.request(asked)
            };

            assert_eq!(
                fetch_git_source(&request).expect("fetch").moving,
                Some(moving),
                "{asked:?} {refresh}"
            );
        }
    }
}

#[test]
fn fetch_checks_a_pin_out_without_resolving_the_ref() {
    let w = world();
    fetch_git_source(&w.request(None)).expect("fetch");
    let moved = advance_the_remote(&w);

    let pinned = GitFetchRequest {
        pin: Some(moved.clone()),
        ..w.request(Some("v1"))
    };

    assert_eq!(fetch_git_source(&pinned).expect("fetch").commit, moved);

    let original = GitFetchRequest {
        pin: Some(w.fixture.commit.clone()),
        ..w.request(Some("main"))
    };

    assert_eq!(
        fetch_git_source(&original).expect("fetch").commit,
        w.fixture.commit
    );
}

#[test]
fn fetch_exits_2_for_a_pin_the_repository_does_not_have() {
    let w = world();
    let request = GitFetchRequest {
        pin: Some("0".repeat(40)),
        ..w.request(None)
    };
    let error = fetch_git_source(&request).expect_err("an unknown pin");

    assert_eq!(error.code, ExitCode::Config);
    assert_eq!(
        error.message,
        "cannot find the locked commit for catalog \"company\" (ambit.yml line 3)"
    );

    let offline = GitFetchRequest {
        offline: true,
        ..request
    };
    let error = fetch_git_source(&offline).expect_err("an uncached pin");

    assert_eq!(error.code, ExitCode::Network);
    assert!(
        error
            .message
            .starts_with("cannot resolve the locked commit from the cache")
    );
}

#[test]
fn recognizes_full_commit_shas_only() {
    assert!(is_commit_sha(&"a".repeat(40)));
    assert!(is_commit_sha(&"A".repeat(64)));
    assert!(!is_commit_sha(&"a".repeat(39)));
    assert!(!is_commit_sha(&"a".repeat(41)));
    assert!(!is_commit_sha(&"g".repeat(40)));
    assert!(!is_commit_sha("main"));
}

#[test]
fn keys_unparseable_and_scp_shapes() {
    assert_eq!(git_cache_key("/srv/git/skills.git"), "local/srv/git/skills");
    assert_eq!(
        git_cache_key("Git.Acme.Test:acme/skills"),
        "git.acme.test/acme/skills"
    );
    assert_eq!(git_cache_key("c:/repos/skills"), "local/c-/repos/skills");
    assert_eq!(
        git_cache_key("https://GitHub.com/a%20b/c d.git"),
        "github.com/a-20b/c-20d"
    );
}

#[test]
fn removes_redirecting_git_variables_from_gits_environment() {
    let env: Env = [
        ("GIT_DIR", "/elsewhere"),
        ("GIT_WORK_TREE", "/elsewhere"),
        ("GIT_INDEX_FILE", "/elsewhere/index"),
        ("KEEP", "1"),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    let copy = git_environment(&env);

    assert_eq!(
        copy.keys().map(String::as_str).collect::<Vec<_>>(),
        ["GIT_TERMINAL_PROMPT", "KEEP"]
    );
}

#[test]
fn reports_a_missing_git_as_exit_4() {
    let root = tempdir();
    let env: Env = [(
        "PATH".to_owned(),
        root.path().to_string_lossy().into_owned(),
    )]
    .into();
    let error = run_git(&["--version"], root.path(), &env).expect_err("no git on PATH");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(error.message, "git is not on PATH");
}
