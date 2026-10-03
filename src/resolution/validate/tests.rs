//! Full-catalog validation: `ambit validate`, one command over one subject.
//!
//! A catalog repo lists **itself**, so the case a separate catalog command once existed for is a
//! project like any other and is asserted here as one; see the catalog-repo cases.
//!
//! Two claims carry this suite. The first is the split from resolution: validation must find what
//! `resolve` deliberately ignores, so the broken-but-unselected cases assert *both* commands: exit
//! 0 from `resolve` and exit 3 from `validate`, on the same catalog.
//!
//! The second is that a CI command lists every problem. Each case that could stop at the first one
//! plants two problems and asserts both, because "exits 3" would pass either way and the whole
//! point of the command is not having to run it six times.
//!
//! Every mutation lands in the per-test copy of the fixture catalog. The shared fixture must stay
//! clean: it is what a golden profile resolves against, and this suite asserts it validates.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;
use crate::errors::{ExitCode, config_error};
use crate::model::catalog::MergedSkill;
use crate::model::config::{ConfigOrigin, ProjectConfig};
use crate::model::pattern::PatternEntry;
use crate::model::requirement::ItemKind;
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::{run_cli, tempdir, test_env};
use crate::util::env::Env;

const CATALOG_NAME: &str = "company";

/// The fixture's shape, which a clean report counts back.
const FIXTURE_PACKS: usize = 4;
const FIXTURE_SKILLS: usize = 4;
const FIXTURE_MCPS: usize = 2;
const FIXTURE_HOOKS: usize = 3;

/// [`Fixture::write_profile`] puts the first `requires` entry here, after the four-line preamble
/// and the key.
const FIRST_ENTRY_LINE: usize = 6;

/// What a clean catalog reports: what was checked, and an explicitly empty problem list.
fn clean_report() -> String {
    [
        format!(
            "checked {FIXTURE_PACKS} packs, {FIXTURE_SKILLS} skills, {FIXTURE_MCPS} mcps, {FIXTURE_HOOKS} hooks"
        ),
        String::new(),
        "problems (0)".to_owned(),
        "  (none)".to_owned(),
    ]
    .join("\n")
}

/// One `requires` entry, taking a whole pack from `catalog`.
fn requires_entry(pack: &str, catalog: &str) -> String {
    format!("  - {{ pack: \"{catalog}/{pack}\" }}")
}

/// One entry of a skill's own `requires`, **unqualified**: the spelling a catalog demands, since
/// the alias in `catalogs:` belongs to the consumer and a catalog author cannot write it.
///
/// By exact name, a pattern with no wildcard, because what these cases are about is which edges
/// the report finds rather than what a glob reaches.
fn needs(kind: &str, name: &str) -> String {
    format!("{{ {kind}: \"{name}\" }}")
}

/// A skill's whole `requires` list as one annotation line, from [`needs`] entries.
fn requires(entries: &[String]) -> String {
    format!("requires: [{}]", entries.join(", "))
}

/// The annotation lines as §3.2 nests them: under a top-level `ambit:`, indented with it.
fn ambit_block(annotations: &[String]) -> Vec<String> {
    if annotations.is_empty() {
        return Vec::new();
    }

    std::iter::once("ambit:".to_owned())
        .chain(annotations.iter().map(|line| format!("  {line}")))
        .collect()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("create the parent");
    fs::write(path, text).expect("write the file");
}

fn strip(text: &str) -> String {
    text.strip_suffix('\n').unwrap_or(text).to_owned()
}

/// What one CLI run printed, each stream's lines joined by `\n` with no trailing newline.
struct Out {
    code: ExitCode,
    stdout: String,
    stderr: String,
}

/// One problem's `kind`, `message`, and `detail`, as `--json` reports them.
#[derive(Debug, PartialEq, Eq)]
struct ProblemRecord {
    kind: String,
    message: String,
    detail: Vec<String>,
}

/// The `--json` report, parsed.
struct Report {
    valid: bool,
    checked: Value,
    problems: Vec<ProblemRecord>,
}

struct Fixture {
    _root: tempfile::TempDir,
    root: PathBuf,
    catalog_dir: PathBuf,
    project_dir: PathBuf,
    env: Env,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempdir();
        let root = dir.path().to_path_buf();
        let fixture = Self {
            catalog_dir: root.join("catalog"),
            project_dir: root.join("project"),
            env: test_env(&root),
            root,
            _root: dir,
        };

        build_fixture_catalog(&fixture.catalog_dir).expect("build the fixture catalog");
        fs::create_dir_all(&fixture.project_dir).expect("create the project");
        fixture.write_profile(&["core"]);
        fixture
    }

    /// Points the project at the fixture catalog and gives it a `requires` list, one `pack:`
    /// entry per pack.
    fn write_profile(&self, packs: &[&str]) {
        let list = if packs.is_empty() {
            "[]".to_owned()
        } else {
            format!(
                "\n{}",
                packs
                    .iter()
                    .map(|pack| requires_entry(pack, CATALOG_NAME))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        };

        write(
            &self.project_dir.join("ambit.yml"),
            &format!(
                "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nrequires: {list}\n"
            ),
        );
    }

    /// Adds a skill to the catalog copy this test owns, its name derived from its path per §2.
    ///
    /// `within` is the catalog root to write it into, so a case about two catalogs providing one
    /// name can put a copy in each.
    fn write_skill_within(within: &Path, relative: &str, annotations: &[String]) {
        let mut text = vec![
            "---".to_owned(),
            format!("name: {}", relative.replace('/', ".")),
        ];

        text.extend(ambit_block(annotations));
        text.extend(["---", "", "# fixture", ""].map(str::to_owned));
        write(
            &within.join("skills").join(relative).join("SKILL.md"),
            &text.join("\n"),
        );
    }

    fn write_skill(&self, relative: &str, annotations: &[String]) {
        Self::write_skill_within(&self.catalog_dir, relative, annotations);
    }

    /// The same, with a frontmatter `name` the path does not derive: the one recoverable
    /// violation.
    fn write_misnamed_skill(&self, relative: &str, declared: &str) {
        write(
            &self
                .catalog_dir
                .join("skills")
                .join(relative)
                .join("SKILL.md"),
            &[
                "---",
                &format!("name: {declared}"),
                "---",
                "",
                "# fixture",
                "",
            ]
            .join("\n"),
        );
    }

    /// Adds a hook, its name derived from its directory per §2.
    fn write_hook(&self, name: &str, lines: &[&str]) {
        let mut text = vec![format!("name: {name}")];

        text.extend(lines.iter().map(|&line| line.to_owned()));
        text.push(String::new());
        write(
            &self
                .catalog_dir
                .join("hooks")
                .join(name.replace('.', "/"))
                .join("hook.yml"),
            &text.join("\n"),
        );
    }

    fn run(&self, args: &[&str], project: &Path) -> Out {
        let project = project.to_string_lossy().into_owned();
        let mut argv: Vec<&str> = args.to_vec();

        argv.extend(["--project", &project]);

        let result = run_cli(&argv, &self.root, &self.env);

        Out {
            code: result.code,
            stdout: strip(&result.stdout),
            stderr: strip(&result.stderr),
        }
    }

    /// Runs the CLI against the project, collecting stdout and stderr.
    fn cli(&self, args: &[&str]) -> Out {
        self.run(args, &self.project_dir)
    }

    /// The same, pointed at the catalog directory rather than at the project: how a catalog
    /// repo's CI invokes the command, once the repo carries the three-line `ambit.yml` that lists
    /// itself.
    fn cli_in_catalog_repo(&self, args: &[&str]) -> Out {
        self.run(args, &self.catalog_dir)
    }

    /// Turns the fixture catalog into a catalog *repo*: what `ambit init` scaffolds for one. No
    /// `requires:` list at all.
    fn write_self_listing_config(&self) {
        write(
            &self.catalog_dir.join("ambit.yml"),
            &[
                "version: 1",
                "catalogs:",
                "  - name: local",
                "    source: path:.",
                "",
            ]
            .join("\n"),
        );
    }

    /// The `--json` report, parsed.
    fn report(&self) -> Report {
        let result = self.cli(&["validate", "--json"]);
        let parsed: Value = serde_json::from_str(&result.stdout).expect("JSON");

        Report {
            valid: parsed["valid"].as_bool().expect("a verdict"),
            checked: parsed["checked"].clone(),
            problems: parsed["problems"]
                .as_array()
                .expect("a problem list")
                .iter()
                .map(|problem| ProblemRecord {
                    kind: problem["kind"].as_str().expect("a kind").to_owned(),
                    message: problem["message"].as_str().expect("a message").to_owned(),
                    detail: problem["detail"]
                        .as_array()
                        .expect("detail lines")
                        .iter()
                        .map(|line| line.as_str().expect("a line").to_owned())
                        .collect(),
                })
                .collect(),
        }
    }
}

fn kinds(report: &Report) -> Vec<&str> {
    report
        .problems
        .iter()
        .map(|problem| problem.kind.as_str())
        .collect()
}

fn s(text: &str) -> String {
    text.to_owned()
}

fn config_for(catalogs: &[&str], requires: Vec<PatternEntry>) -> ProjectConfig {
    ProjectConfig {
        version: 1,
        origin: ConfigOrigin {
            file: s("ambit.yml"),
            entry_lines: indexmap::IndexMap::new(),
        },
        harnesses: vec![s("claude")],
        catalogs: catalogs
            .iter()
            .map(|name| crate::model::config::CatalogRef {
                name: s(name),
                source: format!("path:../{name}"),
                r#ref: None,
            })
            .collect(),
        requires,
    }
}

fn merged_skill(catalog: &str, name: &str) -> MergedSkill {
    MergedSkill {
        name: s(name),
        path: format!("skills/{name}"),
        description: None,
        requires: Vec::new(),
        expects: Vec::new(),
        catalog: s(catalog),
        commit: None,
        catalog_root: PathBuf::from("/catalog"),
    }
}

#[test]
fn a_report_with_no_problems_is_valid() {
    assert!(is_valid(&ValidationReport::default()));
    assert!(!is_valid(&ValidationReport {
        checked: ValidationCounts::default(),
        problems: vec![ValidationProblem {
            kind: ValidationProblemKind::Cycle,
            message: s("requirement cycle"),
            detail: Vec::new(),
        }],
    }));
}

#[test]
fn spells_problem_kinds_as_the_json_reports_them() {
    assert_eq!(
        ValidationProblemKind::ALL
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>(),
        [
            "cycle",
            "name-mismatch",
            "unmatched-pattern",
            "unselected-catalog"
        ]
    );
}

#[test]
fn lists_parsed_problems_first_and_counts_every_copy() {
    let merged = MergedCatalog {
        catalogs: vec![s(CATALOG_NAME), s("personal")],
        skills: vec![
            merged_skill(CATALOG_NAME, "house-style"),
            merged_skill("personal", "house-style"),
        ],
        ..MergedCatalog::default()
    };
    let parsed = problem(
        ValidationProblemKind::NameMismatch,
        config_error("skill name \"x\" does not match its path", ["detail"]),
    );
    let report = validate_catalog(
        &merged,
        &ValidateOptions {
            config: config_for(
                &[CATALOG_NAME, "personal"],
                vec![PatternEntry {
                    kind: ItemKind::Skill,
                    pattern: s("house-style"),
                    catalog: Some(s(CATALOG_NAME)),
                }],
            ),
            parsed: vec![parsed.clone()],
            own: Vec::new(),
        },
    );

    assert_eq!(report.checked.skills, 2);
    assert_eq!(report.problems[0], parsed);
    // `personal` provides an item and nothing is qualified with it.
    assert_eq!(
        report.problems[1].message,
        "catalog \"personal\" is configured but nothing selects from it (ambit.yml)"
    );
    assert_eq!(
        report.problems[1].detail[0],
        "it provides 1 item, and no `requires` entry is qualified with \"personal/\""
    );
    assert_eq!(report.problems.len(), 2);
}

#[test]
fn validate_exits_0_against_the_fixture_catalog_saying_what_it_checked() {
    let fixture = Fixture::new();
    let result = fixture.cli(&["validate"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(result.stdout, clean_report());
}

#[test]
fn validate_checks_a_catalog_repo_which_lists_itself_and_selects_nothing() {
    // A catalog repo is a project that lists its own `skills/`, `mcps/` and `hooks/` as
    // `source: path:.`, and every item in the merged catalog is checked whether anything selects
    // it or not. Nothing here consumes anything (there is no `requires:` list at all) and the
    // whole repo is still checked.
    let fixture = Fixture::new();

    fixture.write_self_listing_config();
    fs::remove_file(fixture.project_dir.join("ambit.yml")).expect("remove ambit.yml");

    let result = fixture.cli_in_catalog_repo(&["validate"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(result.stdout, clean_report());
}

#[test]
fn validate_reports_a_catalog_repos_own_broken_skill_which_nothing_selects() {
    // The report is about items no `requires` entry reaches, which is every item in a catalog
    // repo.
    let fixture = Fixture::new();

    fixture.write_self_listing_config();
    fixture.write_skill(
        "broken-unselected",
        &[requires(&[needs("skill", "absent-skill")])],
    );

    let result = fixture.cli_in_catalog_repo(&["validate"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stdout.contains(
        "`requires` entry \"skill:absent-skill\" matches nothing (skills/broken-unselected/SKILL.md)"
    ));
}

#[test]
fn validate_exits_2_on_a_catalog_that_does_not_parse() {
    // The deliberate boundary: there is no semantic report to build about a document ambit cannot
    // read, so parsing failures stay the exit-2 errors they are everywhere else.
    let fixture = Fixture::new();

    write(
        &fixture.catalog_dir.join("mcps/broken.yml"),
        "name: broken\n",
    );

    let result = fixture.cli(&["validate"]);

    assert_eq!(result.code, ExitCode::Config);
    assert_eq!(result.stdout, "");
    assert!(result.stderr.contains("missing required key \"transport\""));
}

#[test]
fn validate_refuses_a_catalog_that_still_holds_a_scopes_yml() {
    // The registry is gone, and a file that still parses as one would otherwise sit there looking
    // like it labels something.
    let fixture = Fixture::new();

    write(
        &fixture.catalog_dir.join("scopes.yml"),
        "scopes:\n  core:\n    description: A\n",
    );

    let result = fixture.cli(&["validate"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains("the scope registry is gone (scopes.yml)")
    );
    assert!(result.stderr.contains("a group of items is a pack now"));
}

// Spec §4's validation split, in both directions: `resolve` hard-validates the selected closure,
// so a skill nothing selects may carry a dangling `requires`, and `validate` is what refuses to
// let that sit in the catalog.

#[test]
fn validate_reports_a_dangling_requirement_resolve_deliberately_ignores() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "broken-unselected",
        &[requires(&[needs("skill", "absent-skill")])],
    );

    let resolved = fixture.cli(&["resolve"]);
    let validated = fixture.cli(&["validate"]);

    assert_eq!(resolved.code, ExitCode::Success, "{}", resolved.stderr);
    assert_eq!(validated.code, ExitCode::Resolution);
    assert!(validated.stdout.contains(
        "`requires` entry \"skill:absent-skill\" matches nothing (skills/broken-unselected/SKILL.md)"
    ));
    assert!(validated.stdout.contains(&format!(
        "no skill in catalog \"{CATALOG_NAME}\" has a name matching \"absent-skill\""
    )));
}

#[test]
fn validate_reports_an_entry_reaching_no_mcp_entity_naming_the_namespace() {
    let fixture = Fixture::new();

    fixture.write_skill("broken-unselected", &[requires(&[needs("mcp", "absent")])]);

    let found = fixture.report();

    assert_eq!(found.problems.len(), 1);
    assert_eq!(found.problems[0].kind, "unmatched-pattern");
    assert!(found.problems[0].detail[0].contains("no MCP server in catalog \"company\""));
}

#[test]
fn validate_reports_a_missing_hook_by_its_bare_name() {
    let fixture = Fixture::new();

    fixture.write_skill("broken-unselected", &[requires(&[needs("hook", "absent")])]);

    let resolved = fixture.cli(&["resolve"]);
    let found = fixture.report();

    // Nothing selects the skill, so `resolve` never walks the edge and only `validate` reports it.
    assert_eq!(resolved.code, ExitCode::Success, "{}", resolved.stderr);
    assert_eq!(found.problems.len(), 1);
    assert_eq!(found.problems[0].kind, "unmatched-pattern");
    assert!(found.problems[0].detail[0].contains("no hook in catalog \"company\""));
}

#[test]
fn validate_resolves_a_hook_requirement_against_the_hooks_a_catalog_provides() {
    let fixture = Fixture::new();

    fixture.write_hook(
        "guard",
        &["event: Stop", "type: command", "command: npx notify"],
    );
    fixture.write_skill("well-formed", &[requires(&[needs("hook", "guard")])]);

    assert_eq!(fixture.report().problems.len(), 0);
}

#[test]
fn validate_follows_no_edge_out_of_a_hook_when_hunting_cycles() {
    // A hook named like a skill in the cycle would send a one-step walk round it twice; the entry
    // declares its namespace, so the edge reaches the hook and stops there.
    let fixture = Fixture::new();

    fixture.write_hook(
        "cycle-a",
        &["event: Stop", "type: command", "command: npx notify"],
    );
    fixture.write_skill("cycle-a", &[requires(&[needs("hook", "cycle-a")])]);

    assert_eq!(fixture.report().problems.len(), 0);
}

#[test]
fn validate_reports_a_cycle_among_skills_nothing_selects() {
    let fixture = Fixture::new();

    fixture.write_skill("cycle-a", &[requires(&[needs("skill", "cycle-b")])]);
    fixture.write_skill("cycle-b", &[requires(&[needs("skill", "cycle-c")])]);
    fixture.write_skill("cycle-c", &[requires(&[needs("skill", "cycle-a")])]);

    let resolved = fixture.cli(&["resolve"]);
    let validated = fixture.cli(&["validate"]);

    assert_eq!(resolved.code, ExitCode::Success, "{}", resolved.stderr);
    assert_eq!(validated.code, ExitCode::Resolution);
    assert!(validated.stdout.contains("requirement cycle"));
    assert!(
        validated
            .stdout
            .contains("skill:cycle-a → skill:cycle-b → skill:cycle-c → skill:cycle-a")
    );
    assert!(
        validated
            .stdout
            .contains("closed by `skill:cycle-a` in skills/cycle-c/SKILL.md")
    );
    assert!(
        validated
            .stdout
            .contains("break the cycle by removing one `requires` entry")
    );
}

#[test]
fn validate_reports_two_independent_cycles_as_two_problems() {
    let fixture = Fixture::new();

    fixture.write_skill("one-a", &[requires(&[needs("skill", "one-b")])]);
    fixture.write_skill("one-b", &[requires(&[needs("skill", "one-a")])]);
    fixture.write_skill("two-a", &[requires(&[needs("skill", "two-b")])]);
    fixture.write_skill("two-b", &[requires(&[needs("skill", "two-a")])]);

    let found = fixture.report();

    assert_eq!(
        found
            .problems
            .iter()
            .map(|problem| problem.detail[0].as_str())
            .collect::<Vec<_>>(),
        [
            "skill:one-a → skill:one-b → skill:one-a",
            "skill:two-a → skill:two-b → skill:two-a",
        ]
    );
}

#[test]
fn validate_reports_one_loop_once_however_many_skills_lead_into_it() {
    let fixture = Fixture::new();

    fixture.write_skill("entry-left", &[requires(&[needs("skill", "cycle-a")])]);
    fixture.write_skill("entry-right", &[requires(&[needs("skill", "cycle-b")])]);
    fixture.write_skill("cycle-a", &[requires(&[needs("skill", "cycle-b")])]);
    fixture.write_skill("cycle-b", &[requires(&[needs("skill", "cycle-a")])]);

    let found = fixture.report();

    assert_eq!(found.problems.len(), 1);
    assert_eq!(
        found.problems[0].detail[0],
        "skill:cycle-a → skill:cycle-b → skill:cycle-a"
    );
}

#[test]
fn validate_reports_a_dangling_requirement_and_a_cycle_from_one_run() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "broken-dangling",
        &[requires(&[needs("skill", "absent-skill")])],
    );
    fixture.write_skill("cycle-a", &[requires(&[needs("skill", "cycle-b")])]);
    fixture.write_skill("cycle-b", &[requires(&[needs("skill", "cycle-a")])]);

    assert_eq!(kinds(&fixture.report()), ["unmatched-pattern", "cycle"]);
}

// Name↔path agreement.

#[test]
fn validate_lists_a_mismatch_as_a_problem_instead_of_stopping_the_run_at_it() {
    let fixture = Fixture::new();

    fixture.write_misnamed_skill("misnamed-thing", "wrong-name");
    fixture.write_skill(
        "broken-dangling",
        &[requires(&[needs("skill", "absent-skill")])],
    );

    let validated = fixture.cli(&["validate"]);
    let found = fixture.report();

    assert_eq!(validated.code, ExitCode::Resolution);
    // The mismatch alone would be exit 2 anywhere else; here it is one entry of a longer list.
    assert_eq!(kinds(&found), ["name-mismatch", "unmatched-pattern"]);
    assert!(
        found.problems[0]
            .message
            .contains("skill name \"wrong-name\" does not match its path")
    );
    assert_eq!(
        found.problems[0].detail,
        [
            format!("in catalog \"{CATALOG_NAME}\""),
            s("skills/misnamed-thing/SKILL.md derives the name \"misnamed-thing\""),
            s("rename the directory, or correct `name` to match it"),
        ]
    );
}

#[test]
fn validate_goes_on_to_check_the_misnamed_skill_under_the_name_its_path_derives() {
    // Continuing past the mismatch is only worth anything if the rest of the skill is still
    // checked, and the path is the name every other tool would have installed it under.
    let fixture = Fixture::new();

    fixture.write_misnamed_skill("misnamed-thing", "wrong-name");
    fixture.write_skill("needs-it", &[requires(&[needs("skill", "misnamed-thing")])]);

    assert_eq!(kinds(&fixture.report()), ["name-mismatch"]);
}

#[test]
fn a_mismatch_still_exits_2_outside_validation() {
    let fixture = Fixture::new();

    fixture.write_misnamed_skill("misnamed-thing", "wrong-name");

    let result = fixture.cli(&["search", "*"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("does not match its path"));
}

// A name two catalogs provide is not a `validate` finding: nothing is dropped, both copies are
// addressable, and the only place two copies are a conflict is a project that selects both, which
// is resolution's judgement, not a fact about a catalog. So `validate` reports nothing, and
// `resolve` on the very same project refuses.

const SECOND: &str = "personal";

fn with_two_copies() -> Fixture {
    let fixture = Fixture::new();

    // An exact copy of the fixture, so every name is provided twice.
    build_fixture_catalog(&fixture.root.join(SECOND)).expect("build the second catalog");
    write(
        &fixture.project_dir.join("ambit.yml"),
        &[
            s("version: 1"),
            s("catalogs:"),
            format!("  - name: {CATALOG_NAME}"),
            s("    source: path:../catalog"),
            format!("  - name: {SECOND}"),
            format!("    source: path:../{SECOND}"),
            s("requires:"),
            requires_entry("core", CATALOG_NAME),
            requires_entry("core", SECOND),
            s(""),
        ]
        .join("\n"),
    );
    fixture
}

#[test]
fn two_copies_exit_0_counting_every_copy_checked() {
    let fixture = with_two_copies();
    let found = fixture.report();

    assert!(found.valid);
    assert_eq!(found.problems.len(), 0);
    // Both copies, not both names: each is a document this run read and checked on its own terms.
    assert_eq!(
        found.checked,
        json!({
            "hooks": FIXTURE_HOOKS * 2,
            "mcps": FIXTURE_MCPS * 2,
            "packs": FIXTURE_PACKS * 2,
            "skills": FIXTURE_SKILLS * 2,
        })
    );
}

#[test]
fn two_copies_leave_the_collision_to_resolve_which_refuses_the_same_project() {
    // `validate` passes a catalog pair that is perfectly well-formed, and the project selecting
    // both copies is what fails.
    let fixture = with_two_copies();

    assert_eq!(fixture.cli(&["validate"]).code, ExitCode::Success);

    let resolved = fixture.cli(&["resolve"]);

    assert_eq!(resolved.code, ExitCode::Resolution);
    assert!(
        resolved
            .stderr
            .contains("is selected from more than one catalog")
    );
}

#[test]
fn two_copies_of_a_broken_skill_are_reported_once_each() {
    let fixture = with_two_copies();
    let dangling = [requires(&[needs("skill", "absent")])];

    fixture.write_skill("dangling", &dangling);
    Fixture::write_skill_within(&fixture.root.join(SECOND), "dangling", &dangling);

    let found = fixture.report();
    let problems: Vec<&ProblemRecord> = found
        .problems
        .iter()
        .filter(|problem| problem.message.contains("skill:absent"))
        .collect();

    assert_eq!(problems.len(), 2);
    assert_eq!(
        problems
            .iter()
            .map(|problem| problem.kind.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["unmatched-pattern"])
    );
}

#[test]
fn two_copies_do_not_let_one_catalogs_copy_satisfy_the_others_requires() {
    // `personal` ships `needed`, `company` does not, and `company`'s skill asking for it is
    // unsatisfied however plainly the merged view holds a match. A catalog can only require what
    // it ships.
    let fixture = with_two_copies();

    fixture.write_skill("needs-across", &[requires(&[needs("skill", "needed")])]);
    Fixture::write_skill_within(&fixture.root.join(SECOND), "needed", &[]);

    let found = fixture.report();

    assert_eq!(
        found
            .problems
            .iter()
            .map(|problem| problem.message.as_str())
            .collect::<Vec<_>>(),
        ["`requires` entry \"skill:needed\" matches nothing (skills/needs-across/SKILL.md)"]
    );
    assert_eq!(
        found.problems[0].detail,
        [
            format!("no skill in catalog \"{CATALOG_NAME}\" has a name matching \"needed\""),
            s(
                "a catalog's own `requires` resolves within that catalog, which can only require what it ships"
            ),
            s("correct the pattern, add the item to a catalog, or remove the entry"),
        ]
    );
}

// The multi-problem variant of what resolution returns one at a time. Both surfaces use the same
// error builders, so the assertion is that `validate` lists what `resolve` stops at.

#[test]
fn validate_lists_every_entry_that_matches_nothing_in_document_order() {
    let fixture = Fixture::new();

    fixture.write_profile(&["core", "zeta.unknown", "alpha.unknown"]);

    let resolved = fixture.cli(&["resolve"]);
    let found = fixture.report();

    // `resolve` reports the alphabetically first offender and nothing else, so which one it names
    // does not fall to the order the config happens to list them in.
    assert_eq!(resolved.code, ExitCode::Resolution);
    assert!(resolved.stderr.contains(&format!(
        "\"pack:{CATALOG_NAME}/alpha.unknown\" matches nothing"
    )));
    assert!(!resolved.stderr.contains("zeta.unknown"));

    // `validate` lists all of them, down the file the reader has open.
    assert_eq!(
        found
            .problems
            .iter()
            .map(|problem| problem.message.clone())
            .collect::<Vec<_>>(),
        [
            format!(
                "`requires` entry \"pack:{CATALOG_NAME}/zeta.unknown\" matches nothing (ambit.yml line {})",
                FIRST_ENTRY_LINE + 1
            ),
            format!(
                "`requires` entry \"pack:{CATALOG_NAME}/alpha.unknown\" matches nothing (ambit.yml line {})",
                FIRST_ENTRY_LINE + 2
            ),
        ]
    );
    assert_eq!(kinds(&found), ["unmatched-pattern", "unmatched-pattern"]);
}

#[test]
fn validate_checks_the_projects_own_skills_when_it_lists_itself_as_a_catalog() {
    // A project that publishes something is a catalog, so `validate` reads it with no special
    // case, and a broken skill it ships is a finding like any other, whether the project selects it
    // or not.
    let fixture = Fixture::new();

    Fixture::write_skill_within(
        &fixture.project_dir,
        "readwise-cli",
        &[requires(&[needs("skill", "absent-skill")])],
    );
    write(
        &fixture.project_dir.join("ambit.yml"),
        &[
            s("version: 1"),
            s("catalogs:"),
            format!("  - name: {CATALOG_NAME}"),
            s("    source: path:../catalog"),
            s("  - name: local"),
            s("    source: path:."),
            s("requires:"),
            requires_entry("core", CATALOG_NAME),
            s(""),
        ]
        .join("\n"),
    );

    let result = fixture.cli(&["validate"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stdout.contains(&format!(
        "checked {FIXTURE_PACKS} packs, {} skills",
        FIXTURE_SKILLS + 1
    )));
    assert!(result.stdout.contains(
        "`requires` entry \"skill:absent-skill\" matches nothing (skills/readwise-cli/SKILL.md)"
    ));
}

#[test]
fn validate_reports_nothing_about_an_entry_some_configured_catalogs_items_satisfy() {
    let fixture = Fixture::new();

    fixture.write_profile(&[
        "core",
        "function.engineering",
        "function.engineering.*",
        "project.acme",
    ]);

    assert_eq!(fixture.cli(&["validate"]).code, ExitCode::Success);
}

// The one finding whose subject is the config alone: a catalog it lists and never selects from.
//
// This is what a typo'd `source:` escapes into now that a catalog is a directory and nothing else:
// a misspelled path is not a parse failure, and where no pattern is qualified with the alias,
// nothing else has a reason to look at it. Both of its exemptions are here too, because each of
// them is what keeps `validate` passing on something `ambit init` produced.

/// Points the project at the fixture catalog plus one more, selecting only from the fixture.
fn write_two_catalogs(fixture: &Fixture, name: &str, source: &str) {
    write(
        &fixture.project_dir.join("ambit.yml"),
        &[
            s("version: 1"),
            s("catalogs:"),
            format!("  - name: {CATALOG_NAME}"),
            s("    source: path:../catalog"),
            format!("  - name: {name}"),
            format!("    source: {source}"),
            s("requires:"),
            requires_entry("core", CATALOG_NAME),
            s(""),
        ]
        .join("\n"),
    );
}

#[test]
fn unselected_catalog_with_items_no_entry_is_qualified_with_is_reported() {
    let fixture = Fixture::new();

    build_fixture_catalog(&fixture.root.join(SECOND)).expect("build the second catalog");
    write_two_catalogs(&fixture, SECOND, &format!("path:../{SECOND}"));

    assert_eq!(
        fixture.report().problems,
        [ProblemRecord {
            kind: s("unselected-catalog"),
            message: format!(
                "catalog \"{SECOND}\" is configured but nothing selects from it (ambit.yml)"
            ),
            detail: vec![
                format!(
                    "it provides {} items, and no `requires` entry is qualified with \"{SECOND}/\"",
                    FIXTURE_PACKS + FIXTURE_SKILLS + FIXTURE_MCPS + FIXTURE_HOOKS
                ),
                s("select what this project needs from it, or drop it from `catalogs:`"),
            ],
        }]
    );
}

#[test]
fn unselected_catalog_is_reported_as_the_unmatched_pattern_when_an_entry_names_it() {
    // Qualified with, not matched by. An entry spelled `personal/nope` mentions the catalog, so
    // the pattern is the offender and one mistake is reported once.
    let fixture = Fixture::new();

    build_fixture_catalog(&fixture.root.join(SECOND)).expect("build the second catalog");
    write(
        &fixture.project_dir.join("ambit.yml"),
        &[
            s("version: 1"),
            s("catalogs:"),
            format!("  - name: {CATALOG_NAME}"),
            s("    source: path:../catalog"),
            format!("  - name: {SECOND}"),
            format!("    source: path:../{SECOND}"),
            s("requires:"),
            requires_entry("core", CATALOG_NAME),
            requires_entry("nope", SECOND),
            s(""),
        ]
        .join("\n"),
    );

    assert_eq!(kinds(&fixture.report()), ["unmatched-pattern"]);
}

#[test]
fn unselected_catalog_with_no_items_is_just_empty() {
    // `ambit init` scaffolds a live `local` entry against three empty directories and comments out
    // the entry that would select it, so a finding here would fail `validate` on every fresh
    // project.
    let fixture = Fixture::new();

    fs::create_dir_all(fixture.root.join("empty")).expect("create the empty catalog");
    write_two_catalogs(&fixture, "empty", "path:../empty");

    let result = fixture.cli(&["validate"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
}

#[test]
fn unselected_catalog_the_project_itself_is_is_never_reported() {
    // Publishing is not consuming: a repo that ships items and selects none of them is the normal
    // state of a catalog repo, and `ambit init` scaffolded the entry that says so.
    let fixture = Fixture::new();

    Fixture::write_skill_within(&fixture.project_dir, "readwise-cli", &[]);
    write_two_catalogs(&fixture, "local", "path:.");

    let result = fixture.cli(&["validate"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert!(result.stdout.contains(&format!(
        "checked {FIXTURE_PACKS} packs, {} skills",
        FIXTURE_SKILLS + 1
    )));
}

// `ambit validate` output.

#[test]
fn validate_emits_the_problem_list_as_json_with_the_verdict_and_what_was_checked() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "broken-thing",
        &[requires(&[needs("skill", "absent-skill")])],
    );

    let result = fixture.cli(&["validate", "--json"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert_eq!(
        serde_json::from_str::<Value>(&result.stdout).expect("JSON"),
        json!({
            "checked": {
                "hooks": FIXTURE_HOOKS,
                "mcps": FIXTURE_MCPS,
                "packs": FIXTURE_PACKS,
                "skills": FIXTURE_SKILLS + 1,
            },
            "problems": [
                {
                    "detail": [
                        format!("no skill in catalog \"{CATALOG_NAME}\" has a name matching \"absent-skill\""),
                        "a catalog's own `requires` resolves within that catalog, which can only require what it ships",
                        "correct the pattern, add the item to a catalog, or remove the entry",
                    ],
                    "kind": "unmatched-pattern",
                    "message": "`requires` entry \"skill:absent-skill\" matches nothing (skills/broken-thing/SKILL.md)",
                },
            ],
            "valid": false,
        })
    );
}

#[test]
fn validate_emits_valid_true_for_a_clean_catalog_rather_than_an_empty_document() {
    let fixture = Fixture::new();
    let result = fixture.cli(&["validate", "--json"]);

    assert_eq!(
        serde_json::from_str::<Value>(&result.stdout).expect("JSON"),
        json!({
            "checked": {
                "hooks": FIXTURE_HOOKS,
                "mcps": FIXTURE_MCPS,
                "packs": FIXTURE_PACKS,
                "skills": FIXTURE_SKILLS,
            },
            "problems": [],
            "valid": true,
        })
    );
}

#[test]
fn validate_counts_the_hooks_it_checked() {
    let fixture = Fixture::new();

    fixture.write_hook(
        "guard",
        &["event: Stop", "type: command", "command: npx notify"],
    );

    let result = fixture.cli(&["validate"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert!(result.stdout.contains(&format!(
        "checked {FIXTURE_PACKS} packs, {FIXTURE_SKILLS} skills, {FIXTURE_MCPS} mcps, {} hooks",
        FIXTURE_HOOKS + 1
    )));
    assert_eq!(
        fixture.report().checked,
        json!({
            "hooks": FIXTURE_HOOKS + 1,
            "mcps": FIXTURE_MCPS,
            "packs": FIXTURE_PACKS,
            "skills": FIXTURE_SKILLS,
        })
    );
}

#[test]
fn validate_prints_each_problems_detail_indented_under_its_summary() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "broken-thing",
        &[requires(&[needs("skill", "absent-skill")])],
    );

    let result = fixture.cli(&["validate"]);

    assert_eq!(
        result.stdout,
        [
            format!(
                "checked {FIXTURE_PACKS} packs, {} skills, {FIXTURE_MCPS} mcps, {FIXTURE_HOOKS} hooks",
                FIXTURE_SKILLS + 1
            ),
            s(""),
            s("problems (1)"),
            s("  `requires` entry \"skill:absent-skill\" matches nothing (skills/broken-thing/SKILL.md)"),
            format!("      no skill in catalog \"{CATALOG_NAME}\" has a name matching \"absent-skill\""),
            s("      a catalog's own `requires` resolves within that catalog, which can only require what it ships"),
            s("      correct the pattern, add the item to a catalog, or remove the entry"),
        ]
        .join("\n")
    );
}

#[test]
fn validate_emits_byte_identical_json_and_carries_no_machine_paths() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "broken-thing",
        &[requires(&[needs("skill", "absent-skill")])],
    );

    let first = fixture.cli(&["validate", "--json"]);
    let second = fixture.cli(&["validate", "--json"]);

    assert_eq!(second.stdout, first.stdout);
    assert!(!first.stdout.contains(&*fixture.root.to_string_lossy()));
}

#[test]
fn validate_carries_no_machine_paths_in_a_catalog_repo_either() {
    let fixture = Fixture::new();

    fixture.write_self_listing_config();
    fixture.write_misnamed_skill("misnamed-thing", "wrong-name");

    let result = fixture.cli_in_catalog_repo(&["validate"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(!result.stdout.contains(&*fixture.root.to_string_lossy()));
}
