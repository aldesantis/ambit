//! The `read_order` hook is thread-local: every CLI run here must stay in-process on the
//! test's own thread so parallel tests never see each other's permutation.

#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use crate::errors::ExitCode;
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::{CliResult, run_cli, tempdir, test_env};
use crate::util::env::Env;
use crate::util::fs::read_order::{ReadOrder, take_seen, with_read_order};

const CATALOG_NAME: &str = "company";

const CORE_SKILL: &str = "company-context";

const EXTRA_PACK: &str = "determinism.extras";

const SELECTED_PACKS: &[&str] = &[
    "core",
    "function.engineering",
    "function.engineering.frontend",
    "project.acme",
    EXTRA_PACK,
];

const EXTRA_HOOKS: &[(&str, &[&str])] = &[
    (
        "guard",
        &[
            "event: PreToolUse",
            "matcher: Bash",
            "type: command",
            "command: ./bin/guard",
        ],
    ),
    (
        "trace",
        &["event: PreToolUse", "type: command", "command: ./bin/trace"],
    ),
    (
        "notify",
        &["event: Stop", "type: command", "command: ./bin/notify"],
    ),
];

const ENV_STUBS: &[(&str, &str)] = &[
    ("LINTER_API_KEY", "determinism-linter-key"),
    ("FIXTURE_API_KEY", "determinism-fixture-key"),
];

const SHUFFLED: [ReadOrder; 2] = [ReadOrder::Reversed, ReadOrder::Rotated];

#[derive(Clone, Copy, Debug)]
enum Subject {
    Project,
    Empty,
    Reviewed,
}

#[derive(Clone, Copy, Debug)]
struct Surface {
    argv: &'static [&'static str],
    dir: Subject,
}

impl Surface {
    fn title(&self) -> String {
        let dir = match self.dir {
            Subject::Project => "project",
            Subject::Empty => "empty",
            Subject::Reviewed => "reviewed",
        };

        format!("ambit {} --project <{dir}>", self.argv.join(" "))
    }
}

macro_rules! surfaces {
    ($($name:ident: [$($arg:expr),+] in $dir:ident;)+) => {
        const SURFACES: &[Surface] = &[$(Surface { argv: &[$($arg),+], dir: Subject::$dir }),+];

        mod twice {
            #[allow(clippy::wildcard_imports)]
            use super::*;

            $(
                #[test]
                fn $name() {
                    prints_the_same_bytes_twice(Surface {
                        argv: &[$($arg),+],
                        dir: Subject::$dir,
                    });
                }
            )+
        }

        mod shuffled {
            #[allow(clippy::wildcard_imports)]
            use super::*;

            $(
                #[test]
                fn $name() {
                    ignores_the_read_order(Surface {
                        argv: &[$($arg),+],
                        dir: Subject::$dir,
                    });
                }
            )+
        }
    };
}

surfaces! {
    init_dry_run: ["init", "--dry-run"] in Empty;
    init_dry_run_json: ["init", "--dry-run", "--json"] in Empty;
    search: ["search", "*"] in Project;
    search_json: ["search", "*", "--json"] in Project;
    resolve: ["resolve"] in Project;
    resolve_json: ["resolve", "--json"] in Project;
    resolve_explain: ["resolve", "--explain"] in Project;
    resolve_explain_json: ["resolve", "--explain", "--json"] in Project;
    why_skill: ["why", CORE_SKILL] in Project;
    why_skill_json: ["why", CORE_SKILL, "--json"] in Project;
    why_mcp: ["why", "mcp.fixture"] in Project;
    why_hook: ["why", "hook.guard"] in Project;
    status: ["status"] in Project;
    status_json: ["status", "--json"] in Project;
    validate: ["validate"] in Project;
    validate_json: ["validate", "--json"] in Project;
    doctor: ["doctor"] in Project;
    doctor_json: ["doctor", "--json"] in Project;
    audit: ["audit"] in Project;
    audit_json: ["audit", "--json"] in Project;
    install_dry_run: ["install", "--dry-run"] in Project;
    install_dry_run_json: ["install", "--dry-run", "--json"] in Project;
    prune_dry_run: ["prune", "--dry-run"] in Project;
    prune_dry_run_json: ["prune", "--dry-run", "--json"] in Project;
    clean_dry_run: ["clean", "--dry-run"] in Project;
    clean_dry_run_json: ["clean", "--dry-run", "--json"] in Project;
    outdated: ["outdated"] in Project;
    outdated_json: ["outdated", "--json"] in Project;
    update_dry_run: ["update", "--dry-run"] in Project;
    update_dry_run_json: ["update", "--dry-run", "--json"] in Project;
    install_dry_run_refused: ["install", "--dry-run"] in Reviewed;
    install_dry_run_refused_json: ["install", "--dry-run", "--json"] in Reviewed;
}

struct Fixture {
    root: tempfile::TempDir,
    env: Env,
    catalog: PathBuf,
    project: PathBuf,
    empty: PathBuf,
    reviewed: PathBuf,
    installed: BTreeMap<String, String>,
    built: BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempdir();
        let env = env_for(root.path());
        let catalog = root.path().join("catalog");
        let project = root.path().join("project");
        let empty = root.path().join("empty");
        let reviewed = root.path().join("reviewed");

        build_fixture_catalog(&catalog).expect("build the fixture catalog");
        write_extra_hooks(&catalog);
        fs::create_dir_all(&project).expect("create the project");
        fs::create_dir_all(&empty).expect("create the empty directory");
        fs::create_dir_all(&reviewed).expect("create the reviewed project");
        write_profile(&project);
        write_reviewed_profile(&reviewed);

        let install = cli(
            &["install", "--project", path_str(&project)],
            root.path(),
            &env,
        );

        assert_eq!(install.code, ExitCode::Success, "{}", install.stderr);

        let installed = snapshot(&project);
        let built = snapshot(&catalog);

        Self {
            root,
            env,
            catalog,
            project,
            empty,
            reviewed,
            installed,
            built,
        }
    }

    fn dir_of(&self, subject: Subject) -> &Path {
        match subject {
            Subject::Project => &self.project,
            Subject::Empty => &self.empty,
            Subject::Reviewed => &self.reviewed,
        }
    }

    fn run(&self, surface: Surface, order: ReadOrder) -> CliResult {
        let mut argv: Vec<&str> = surface.argv.to_vec();

        argv.extend(["--project", path_str(self.dir_of(surface.dir))]);
        with_read_order(order, || cli(&argv, self.root.path(), &self.env))
    }
}

fn env_for(root: &Path) -> Env {
    let mut env = test_env(root);

    for (name, value) in ENV_STUBS {
        env.insert((*name).to_owned(), (*value).to_owned());
    }

    env
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 temporary path")
}

fn cli(argv: &[&str], cwd: &Path, env: &Env) -> CliResult {
    run_cli(argv, cwd, env)
}

fn requires_entry(pack: &str) -> String {
    format!("  - {{ pack: \"{CATALOG_NAME}/{pack}\" }}")
}

fn write_profile(dir: &Path) {
    let entries: Vec<String> = SELECTED_PACKS
        .iter()
        .map(|pack| requires_entry(pack))
        .collect();

    fs::write(
        dir.join("ambit.yml"),
        format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nrequires:\n{}\n",
            entries.join("\n")
        ),
    )
    .expect("write ambit.yml");
}

fn write_reviewed_profile(dir: &Path) {
    write_profile(dir);

    let config = dir.join("ambit.yml");
    let text = fs::read_to_string(&config).expect("read ambit.yml");

    fs::write(
        &config,
        text.replace(
            "    source: path:../catalog\n",
            "    source: path:../catalog\n    trust: review\n",
        ),
    )
    .expect("write ambit.yml");
}

fn write_extra_hooks(dir: &Path) {
    for (name, lines) in EXTRA_HOOKS {
        let target = dir.join("hooks").join(name);
        let mut text = vec![format!("name: {name}")];

        text.extend(lines.iter().map(|&line| line.to_owned()));
        text.push(String::new());
        fs::create_dir_all(&target).expect("create the hook directory");
        fs::write(target.join("hook.yml"), text.join("\n")).expect("write hook.yml");
    }

    let (group, leaf) = EXTRA_PACK.split_once('.').expect("a dotted pack name");
    let mut text = vec![
        format!("name: {EXTRA_PACK}"),
        "description: The hooks this determinism suite writes, so one entry selects them all."
            .to_owned(),
        "requires:".to_owned(),
    ];

    text.extend(
        EXTRA_HOOKS
            .iter()
            .map(|(name, _)| format!("  - hook: {name}")),
    );
    text.push(String::new());
    fs::create_dir_all(dir.join("packs").join(group)).expect("create the pack directory");
    fs::write(
        dir.join("packs").join(group).join(format!("{leaf}.yml")),
        text.join("\n"),
    )
    .expect("write the pack");
}

/// Deliberately does not follow links, and walks with `std::fs::read_dir` so the snapshot
/// neither records nor permutes a listing.
fn snapshot(dir: &Path) -> BTreeMap<String, String> {
    fn walk(current: &Path, relative: &str, found: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(current).expect("list a directory") {
            let entry = entry.expect("read a directory entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let within = if relative.is_empty() {
                name
            } else {
                format!("{relative}/{name}")
            };
            let absolute = entry.path();
            let kind = fs::symlink_metadata(&absolute).expect("lstat").file_type();

            if kind.is_symlink() {
                let target = fs::read_link(&absolute).expect("readlink");

                found.insert(within, format!("-> {}", target.display()));
            } else if kind.is_dir() {
                walk(&absolute, &within, found);
            } else {
                let bytes = fs::read(&absolute).expect("read a file");

                found.insert(within, String::from_utf8_lossy(&bytes).into_owned());
            }
        }
    }

    let mut found = BTreeMap::new();

    walk(dir, "", &mut found);
    found
}

fn prints_the_same_bytes_twice(surface: Surface) {
    let fixture = Fixture::new();
    let first = fixture.run(surface, ReadOrder::Natural);
    let second = fixture.run(surface, ReadOrder::Natural);

    assert_eq!(second, first, "{}", surface.title());
}

fn ignores_the_read_order(surface: Surface) {
    let fixture = Fixture::new();
    let baseline = fixture.run(surface, ReadOrder::Natural);

    for order in SHUFFLED {
        assert_eq!(
            fixture.run(surface, order),
            baseline,
            "{}, read order: {order:?}",
            surface.title()
        );
    }
}

#[test]
fn hands_a_directorys_entries_back_in_a_different_order_under_each_shuffle() {
    let dir = tempdir();

    for name in ["a", "b", "c"] {
        fs::write(dir.path().join(name), "").expect("write an entry");
    }

    let list = |order| {
        with_read_order(order, || {
            crate::util::fs::read_dir_names(dir.path()).expect("list the directory")
        })
    };
    let natural = list(ReadOrder::Natural);
    let reversed = list(ReadOrder::Reversed);
    let rotated = list(ReadOrder::Rotated);

    let mut expected_reversed = natural.clone();
    expected_reversed.reverse();
    assert_eq!(reversed, expected_reversed);
    assert_ne!(reversed, natural);
    assert_ne!(rotated, natural);
    assert_ne!(rotated, reversed);

    let (mut sorted_rotated, mut sorted_natural) = (rotated.clone(), natural.clone());
    sorted_rotated.sort();
    sorted_natural.sort();
    assert_eq!(sorted_rotated, sorted_natural);
}

#[test]
fn permutes_the_listings_ambit_reads_not_only_the_ones_this_file_reads() {
    let fixture = Fixture::new();

    take_seen();

    let result = fixture.run(
        Surface {
            argv: &["resolve", "--json"],
            dir: Subject::Project,
        },
        ReadOrder::Reversed,
    );

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

    let seen = take_seen();

    for half in ["skills", "mcps", "packs"] {
        let dir = fixture.catalog.join(half);

        assert!(
            seen.iter().any(|listed| listed == &dir),
            "{} was not listed through the hook: {seen:?}",
            dir.display()
        );
    }
}

#[test]
fn names_no_absolute_path_from_this_machine() {
    let fixture = Fixture::new();
    let root = fixture.root.path().to_string_lossy().into_owned();
    let canonical = fs::canonicalize(fixture.root.path())
        .expect("canonicalize the root")
        .to_string_lossy()
        .into_owned();
    let tmp = std::env::temp_dir().to_string_lossy().into_owned();

    for surface in SURFACES {
        let result = fixture.run(*surface, ReadOrder::Natural);
        let printed = format!("{}\n{}", result.stdout, result.stderr);

        for machine in [&root, &canonical, &tmp] {
            assert!(
                !printed.contains(machine.as_str()),
                "{} printed {machine}:\n{printed}",
                surface.title()
            );
        }
    }
}

#[test]
fn prints_no_date_and_no_clock_time() {
    let fixture = Fixture::new();
    let timestamp = Regex::new(r"\d{4}-\d{2}-\d{2}|\d{2}:\d{2}:\d{2}").expect("a valid regex");

    for surface in SURFACES {
        let result = fixture.run(*surface, ReadOrder::Natural);
        let printed = format!("{}\n{}", result.stdout, result.stderr);

        assert!(
            !timestamp.is_match(&printed),
            "{}:\n{printed}",
            surface.title()
        );
    }
}

#[test]
fn the_reviewed_project_is_refused_by_the_execution_gate() {
    let fixture = Fixture::new();
    let result = fixture.run(
        Surface {
            argv: &["install", "--dry-run"],
            dir: Subject::Reviewed,
        },
        ReadOrder::Natural,
    );

    assert_eq!(result.code, ExitCode::Drift, "{}", result.stderr);
    assert!(
        result
            .stderr
            .starts_with("error: install would add execution that was not in the lock"),
        "{}",
        result.stderr
    );
}

#[test]
fn nothing_in_the_surface_table_touches_disk() {
    let fixture = Fixture::new();

    for surface in SURFACES {
        fixture.run(*surface, ReadOrder::Natural);
    }

    assert_eq!(
        snapshot(&fixture.project),
        fixture.installed,
        "leaves the installed project byte-identical to what install wrote"
    );
    assert_eq!(
        snapshot(&fixture.catalog),
        fixture.built,
        "leaves the catalog byte-identical to what the fixture builder wrote"
    );
    assert_eq!(
        snapshot(&fixture.empty),
        BTreeMap::new(),
        "leaves the empty directory empty, which is what `init --dry-run` promises"
    );
    assert_eq!(
        snapshot(&fixture.reviewed).into_keys().collect::<Vec<_>>(),
        ["ambit.yml"],
        "leaves the reviewed project holding only its config, as a refused install must"
    );
}

struct BrokenCatalog {
    root: tempfile::TempDir,
    env: Env,
    catalog: PathBuf,
}

impl BrokenCatalog {
    fn new() -> Self {
        let root = tempdir();
        let env = env_for(root.path());
        let catalog = root.path().join("catalog");

        build_fixture_catalog(&catalog).expect("build the fixture catalog");
        fs::write(
            catalog.join("ambit.yml"),
            "version: 1\ncatalogs:\n  - name: local\n    source: path:.\n",
        )
        .expect("write ambit.yml");

        Self { root, env, catalog }
    }

    fn write_mismatched_skill(&self, relative: &str, declared: &str) {
        let target = self.catalog.join("skills").join(relative);

        fs::create_dir_all(&target).expect("create the skill directory");
        fs::write(
            target.join("SKILL.md"),
            format!("---\nname: {declared}\n---\n\n# fixture\n"),
        )
        .expect("write SKILL.md");
    }

    fn validate(&self, order: ReadOrder) -> CliResult {
        with_read_order(order, || {
            cli(
                &["validate", "--project", path_str(&self.catalog)],
                self.root.path(),
                &self.env,
            )
        })
    }

    fn assert_stable(&self, expected: &CliResult) {
        for order in SHUFFLED {
            assert_eq!(&self.validate(order), expected, "read order: {order:?}");
        }
    }
}

#[test]
fn lists_two_skills_whose_names_disagree_with_their_paths_in_one_order() {
    let broken = BrokenCatalog::new();

    broken.write_mismatched_skill("broken-alpha", "wrong-alpha");
    broken.write_mismatched_skill("broken-omega", "wrong-omega");

    let baseline = broken.validate(ReadOrder::Natural);

    assert_eq!(baseline.code, ExitCode::Resolution);
    assert!(
        baseline.stdout.contains("problems (2)"),
        "{}",
        baseline.stdout
    );
    let alpha = baseline
        .stdout
        .find("broken-alpha")
        .expect("alpha reported");
    let omega = baseline
        .stdout
        .find("broken-omega")
        .expect("omega reported");
    assert!(alpha < omega, "{}", baseline.stdout);
    broken.assert_stable(&baseline);
}

#[test]
fn names_the_two_documents_defining_one_mcp_entity_in_one_order() {
    let broken = BrokenCatalog::new();

    for extension in [".yml", ".yaml"] {
        fs::write(
            broken.catalog.join("mcps").join(format!("dup{extension}")),
            "name: dup\ntransport:\n  stdio:\n    command: fixture-mcp\n",
        )
        .expect("write a duplicate server");
    }

    let baseline = broken.validate(ReadOrder::Natural);

    assert_eq!(baseline.code, ExitCode::Config);
    assert!(
        baseline
            .stderr
            .contains("mcps/dup.yaml and mcps/dup.yml both define \"dup\""),
        "{}",
        baseline.stderr
    );
    broken.assert_stable(&baseline);
}

#[test]
fn ambit_install_puts_identical_bytes_and_identical_links_in_every_project() {
    let root = tempdir();
    let env = env_for(root.path());
    let catalog = root.path().join("catalog");

    build_fixture_catalog(&catalog).expect("build the fixture catalog");
    write_extra_hooks(&catalog);

    let install_under = |order: ReadOrder| {
        let dir = root.path().join(format!("project-{order:?}"));

        fs::create_dir_all(&dir).expect("create the project");
        write_profile(&dir);

        let result = with_read_order(order, || {
            cli(&["install", "--project", path_str(&dir)], root.path(), &env)
        });

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        snapshot(&dir)
    };

    let baseline = install_under(ReadOrder::Natural);

    for order in SHUFFLED {
        assert_eq!(install_under(order), baseline, "read order: {order:?}");
    }
}
