//! Determinism, as a suite.
//!
//! Determinism is a requirement rather than a preference: sort every collection before iterating,
//! never depend on map order, never emit a timestamp, never let filesystem read order reach output.
//! Each module's tests assert their own corner of it (`ambit.lock`'s bytes,
//! `resolve --explain --json`, the catalog report, both scaffolds). This file is the systematic
//! version. One table lists every surface ambit prints; every surface is run twice, then run again
//! with every directory listing permuted, and the bytes must not move. **Adding a command means
//! adding a row to [`surfaces!`]**, which is the whole of extending this file.
//!
//! Read order is permuted through [`util::fs::read_order`](crate::util::fs::read_order), not by
//! rebuilding the fixture in a different order. The order a filesystem hands entries back in is not
//! something a test can arrange (APFS answers in one stable hash order, ext4 in another, and neither
//! is the creation order a test could shuffle), so a hook in the one directory-listing function is
//! the only way to make the second determinism claim testable at all. That makes the hook
//! load-bearing: a suite whose shuffle quietly stopped applying would pass forever, so the first
//! cases below prove the permutation reaches the listings *ambit* reads. Clippy's
//! `disallowed-methods` on `std::fs::read_dir` is what keeps every listing going through it.
//!
//! Nothing in the surface table writes, so each case shares one installed project, one catalog and
//! one empty directory among its surfaces; the "wrote nothing" case is what pins that, and it is
//! the reason the `--dry-run` previews are safe to list beside the read-only commands.
//!
//! One thing the table cannot do on its own: every surface in it is sorted twice over (a catalog's
//! directory entries as they are read, its items again before they are emitted), so a single
//! missing sort moves none of those bytes. The "report of problems" cases near the end are where
//! the shuffle bites, because a problem list and a duplicate-stem refusal are in *found* order and
//! have nothing but the entry sort protecting them. Read the two together: the table says the
//! surfaces are stable, and those cases say the mechanism keeping them stable is still there.
//!
//! The hook is thread-local and every CLI run here is in-process on the test's own thread, so
//! parallel test threads never see each other's permutation.

#![allow(clippy::disallowed_methods)] // The snapshot walks with std::fs on purpose: see `snapshot`.

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

/// The pack this file writes to gather [`EXTRA_HOOKS`], since nothing labels itself any more.
const EXTRA_PACK: &str = "determinism.extras";

/// Enough packs to select every skill and server the fixture holds, so each surface has as much to
/// sort as it can. The third is a descendant of the second by name, which takes an entry of its
/// own: a name is a name, and there is no subtree rule that would reach it implicitly.
///
/// The last is this file's own, written beside the extra hooks below.
const SELECTED_PACKS: &[&str] = &[
    "core",
    "function.engineering",
    "function.engineering.frontend",
    "project.acme",
    EXTRA_PACK,
];

/// Three hooks written into the catalog copy this file owns.
///
/// Beyond the three the fixture ships, and arranged so both orderings a hook config file has are
/// non-trivial: two share an event, so an array's own order has to come from the bundle, and the
/// event keys are written in an order that is not the order the names sort in. All three are
/// gathered into [`EXTRA_PACK`], which the project selects.
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

/// The fixture's two credentials, stubbed so no surface depends on the developer's environment.
const ENV_STUBS: &[(&str, &str)] = &[
    ("LINTER_API_KEY", "determinism-linter-key"),
    ("FIXTURE_API_KEY", "determinism-fixture-key"),
];

/// The orders every claim here is repeated under, beyond the filesystem's own.
const SHUFFLED: [ReadOrder; 2] = [ReadOrder::Reversed, ReadOrder::Rotated];

/// Which of the two directories a surface is pointed at.
///
/// Both are named by `--project`, the only directory flag there is. `Empty` is a project directory
/// with nothing in it, which exists for the one surface whose subject is the *absence* of a
/// project: `ambit init` refuses a directory that already holds a config, so it cannot be aimed at
/// `Project` like the rest.
#[derive(Clone, Copy, Debug)]
enum Subject {
    Project,
    Empty,
}

/// One thing ambit prints, and which directory it is pointed at.
#[derive(Clone, Copy, Debug)]
struct Surface {
    /// The words a user types, without the directory flag.
    argv: &'static [&'static str],
    dir: Subject,
}

impl Surface {
    /// How a surface's case is titled: what a reader would have to type to reproduce it.
    fn title(&self) -> String {
        let dir = match self.dir {
            Subject::Project => "project",
            Subject::Empty => "empty",
        };

        format!("ambit {} --project <{dir}>", self.argv.join(" "))
    }
}

/// Declares [`SURFACES`] and, for every row, one case per claim, so a failure names the surface.
///
/// Text and `--json` are separate rows on purpose: they are two renderings, and only one of them is
/// covered by the goldens. The `--dry-run` rows are here because a preview is a report: the one
/// surface of a mutating command that prints without writing, and the one nothing else asserts
/// twice. They preview a project's init, install, prune and clean, which is every command that
/// writes.
///
/// The project's one catalog is a `path:` source, so `outdated` and `update --dry-run` reach no
/// remote and report it as `unversioned`, which is the case worth pinning here rather than in spite
/// of it: a report whose rows depend on nothing outside the fixture is exactly what a determinism
/// table can assert.
macro_rules! surfaces {
    ($($name:ident: [$($arg:expr),+] in $dir:ident;)+) => {
        /// Every surface whose bytes this file pins.
        const SURFACES: &[Surface] = &[$(Surface { argv: &[$($arg),+], dir: Subject::$dir }),+];

        /// Every surface prints the same bytes twice.
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

        /// Every surface ignores the order the filesystem lists directories in.
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
}

/// One installed project beside its catalog, and an empty directory, under one temporary root.
struct Fixture {
    root: tempfile::TempDir,
    env: Env,
    catalog: PathBuf,
    project: PathBuf,
    empty: PathBuf,
    /// The project as install left it.
    installed: BTreeMap<String, String>,
    /// The catalog as the fixture builder left it.
    built: BTreeMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempdir();
        let env = env_for(root.path());
        let catalog = root.path().join("catalog");
        let project = root.path().join("project");
        let empty = root.path().join("empty");

        build_fixture_catalog(&catalog).expect("build the fixture catalog");
        write_extra_hooks(&catalog);
        fs::create_dir_all(&project).expect("create the project");
        fs::create_dir_all(&empty).expect("create the empty directory");
        write_profile(&project);

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
            installed,
            built,
        }
    }

    fn dir_of(&self, subject: Subject) -> &Path {
        match subject {
            Subject::Project => &self.project,
            Subject::Empty => &self.empty,
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

/// One `requires` entry, taking a whole pack from the catalog.
fn requires_entry(pack: &str) -> String {
    format!("  - {{ pack: \"{CATALOG_NAME}/{pack}\" }}")
}

/// Points a project at a sibling `catalog/` directory and takes every pack the fixture declares.
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

/// Adds [`EXTRA_HOOKS`] to the catalog, and the pack that gathers them.
///
/// The pack is nested one directory deep, so this file's own catalog edit exercises the `packs/**`
/// walk rather than only the flat case.
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

/// Every path under `dir`, relative and `/`-separated, mapped to what is there: a file's bytes, or
/// `-> target` for a symlink.
///
/// Deliberately does not follow a link. Which of the two shapes install chose is part of what has
/// to stay stable, and descending into a linked skill would compare the catalog's own bytes
/// instead. Walks with `std::fs::read_dir`, so the snapshot itself neither records nor permutes a
/// listing.
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

// The shuffled read order this suite relies on. Both cases fail loudly rather than vacuously: the
// first if the permutation stops permuting, the second if it stops reaching ambit's own listings.

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

    // Every half of a catalog is walked through the hook, so a sort removed from any one would
    // show up in the cases below rather than passing unobserved.
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

// No surface carries anything machine-specific: output that named a machine path or the wall
// clock would differ between two machines even though it is stable on one, which is the failure a
// golden file cannot catch.

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

// Nothing in the surface table touches disk: the guard on sharing one project and one catalog
// across every surface. Each row is either read-only or a `--dry-run`, and a row that turned out
// to write would have corrupted the fixture for whatever ran after it.

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
}

/// A broken *catalog repo*: a copy of the fixture carrying the three-line `ambit.yml` that lists
/// itself, which is how a catalog is validated.
///
/// The two cases using it are what make the shuffle more than a formality. Everything the table
/// asserts is protected twice over, so a single missing sort would not move any of those bytes.
/// These two do move: a report of *problems* is in the order they were found, and the two documents
/// in a duplicate-stem refusal are named in the order the directory listed them, so nothing but the
/// entry sort stands between read order and output. Both are `ambit validate`, the surface that
/// reports rather than throws on the first offender. Every catalog here is a per-test copy: the
/// shared fixture has to stay valid.
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
        // What makes the directory a project as well as a catalog, and so `ambit validate`'s
        // subject.
        fs::write(
            catalog.join("ambit.yml"),
            "version: 1\ncatalogs:\n  - name: local\n    source: path:.\n",
        )
        .expect("write ambit.yml");

        Self { root, env, catalog }
    }

    /// Adds a skill whose frontmatter `name` disagrees with its path: the one problem parsing
    /// collects.
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

    /// Asserts the two shuffles print exactly what the filesystem's own order printed.
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
    // The report is in the order the walk found them, so the sort has to be in the walk.
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

/// The write path's own determinism: the whole installed tree (lock, state, `.mcp.json`, the
/// gitignore block, and every symlink target) comes out the same when the catalog's directories are
/// read in a different order.
///
/// Each order installs into its own project directory, all of them siblings of one catalog, so the
/// relative symlinks a linked skill carries are comparable between them.
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
