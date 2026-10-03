//! Resolution by pattern, and the `ambit resolve` output built on it.
//!
//! The rule under test is that one grammar does all the selecting: an entry is one key naming a
//! namespace and carrying the glob to match names in it, and an exact name is a glob with no
//! wildcard. So the cases here are about what an entry reaches and, just as much, about what it
//! does not: `core.*` excluding `core`, a qualifier confining an entry to one catalog, and a
//! `skill:` entry never reaching a hook of the same name.
//!
//! Grouping is a **pack**: a catalog document whose `requires` names the items it gathers, and
//! which a project takes with one entry. The closure that expands one is the same closure a
//! skill's own `requires` goes through, so the two are tested together rather than as two
//! mechanisms.
//!
//! The `resolve --json` shape is pinned by golden files under `tests/golden/resolve/`, driven from
//! `tests/resolve_golden.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use pretty_assertions::assert_eq;
use serde_json::Value;

use super::*;
use crate::errors::ExitCode;
use crate::model::catalog::{CatalogLoadOptions, load_catalogs, merge_catalogs};
use crate::model::config::load_project_config;
use crate::model::sources::SourceContext;
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::{run_cli, tempdir, test_env};
use crate::util::env::Env;
use crate::util::text::pad_end;

const CATALOG_NAME: &str = "company";

const CORE_SKILL: &str = "company-context";
const ENGINEERING_SKILL: &str = "code-review";
const FRONTEND_SKILL: &str = "design-tokens";
const PROJECT_SKILL: &str = "acme-brief";

/// The fixture's deepest pack name, which is what a section's first column pads out to.
const FRONTEND_PACK: &str = "function.engineering.frontend";

/// The fixture's two packed hooks, in the sections they appear in; both names 13 wide.
const CORE_HOOK: &str = "session-notes";
const ENGINEERING_HOOK: &str = "guard-secrets";

/// The hooks the fixture itself ships.
const FIXTURE_HOOKS: &[&str] = &[CORE_HOOK, ENGINEERING_HOOK, "acme-standup"];

/// The line [`Fixture::write_profile`] puts the first `requires` entry on, after the four-line
/// preamble and the key itself.
///
/// Named because the exact line is what a refusal is asserted to print: the whole point of the
/// message is that it sends a reader to the offending line of their own config, so a loose
/// assertion would pass on a message that named the file alone.
const FIRST_ENTRY_LINE: usize = 6;

/// One `requires` entry as a single config line.
///
/// An entry is one key, so it occupies one line and the position a refusal names is countable from
/// the profile's four-line preamble. The address is qualified with the fixture catalog unless it
/// already carries a qualifier of its own.
fn entry(kind: &str, address: &str) -> String {
    let qualified = if address.contains('/') {
        address.to_owned()
    } else {
        format!("{CATALOG_NAME}/{address}")
    };

    format!("  - {{ {kind}: \"{qualified}\" }}")
}

/// One entry of a skill's own `requires`, **unqualified**: the spelling a catalog demands, since
/// the alias in `catalogs:` belongs to the consumer and a catalog author cannot write it.
///
/// By exact name, which is a pattern with no wildcard: most cases here are about what the closure
/// does with an edge rather than about what a glob reaches, and `pattern` owns the matcher.
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

/// The name↔path convention: the path under `skills/` with `/` → `.`.
fn name_from_path(relative: &str) -> String {
    relative.replace('/', ".")
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a parent")).expect("create the parent");
    fs::write(path, text).expect("write the file");
}

fn skill_document(relative: &str, annotations: &[String]) -> String {
    let mut lines = vec![
        "---".to_owned(),
        format!("name: {}", name_from_path(relative)),
    ];

    lines.extend(ambit_block(annotations));
    lines.extend(["---", "", "# fixture", ""].map(str::to_owned));
    lines.join("\n")
}

fn lines(items: &[&str]) -> Vec<String> {
    items.iter().map(|&line| line.to_owned()).collect()
}

/// What one CLI run printed, each stream's lines joined by `\n` with no trailing newline.
struct Out {
    code: ExitCode,
    stdout: String,
    stderr: String,
}

fn strip(text: &str) -> String {
    text.strip_suffix('\n').unwrap_or(text).to_owned()
}

/// A temporary root holding the fixture catalog (`catalog/`) and a project (`project/`) pointed
/// at it.
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
        fixture.write_profile(&[entry("pack", "core")], &[]);
        fixture
    }

    /// Points the project at the fixture catalog and gives it a `requires` list.
    ///
    /// `extra` holds further top-level config lines (a deleted key, for the cases that refuse
    /// one), appended after the list, so the line each entry sits on does not depend on them.
    fn write_profile(&self, requires: &[String], extra: &[&str]) {
        let list = if requires.is_empty() {
            "[]".to_owned()
        } else {
            format!("\n{}", requires.join("\n"))
        };
        let extra: String = extra.iter().flat_map(|line| [line, "\n"]).collect();

        write(
            &self.project_dir.join("ambit.yml"),
            &format!(
                "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nrequires: {list}\n{extra}"
            ),
        );
    }

    /// Adds an MCP entity to the fixture catalog, its name taken from its filename per §3.3.
    ///
    /// Only the cases the shared fixture cannot hold need this: a server whose name collides with
    /// a skill's, which no sane catalog would ship.
    fn write_mcp(&self, name: &str, annotations: &[&str]) {
        let mut text = vec![format!("name: {name}")];

        text.extend(annotations.iter().map(|&line| line.to_owned()));
        text.extend(lines(&[
            "transport:",
            "  stdio:",
            "    command: fixture-mcp",
            "",
        ]));
        write(
            &self.catalog_dir.join("mcps").join(format!("{name}.yml")),
            &text.join("\n"),
        );
    }

    /// Adds a pack to the fixture catalog, gathering `entries`.
    ///
    /// The other half of [`Fixture::in_core_pack`]: where that edits a pack the profile already
    /// takes, this one declares a new grouping for a case that wants to select it by name.
    fn write_pack(&self, name: &str, entries: &[String]) {
        let mut text = vec![
            format!("name: {name}"),
            format!("description: The {name} pack, written by a test."),
            "requires:".to_owned(),
        ];

        text.extend(entries.iter().map(|line| format!("  - {line}")));
        text.push(String::new());
        write(
            &self.catalog_dir.join("packs").join(format!("{name}.yml")),
            &text.join("\n"),
        );
    }

    /// Rewrites the fixture's `core` pack so it gathers `entries` as well as its own two members.
    ///
    /// A grouping is a document, so a case that needs its own skills in the bundle edits the pack
    /// the profile takes, which is the edit an author would make in a real catalog, and the reason
    /// a misspelling here is a resolution error rather than a new label reaching nobody.
    fn in_core_pack(&self, entries: &[String]) {
        let mut text = lines(&[
            "name: core",
            "description: What every Acme session needs, whoever is in it.",
            "requires:",
        ]);

        text.extend(
            [needs("skill", CORE_SKILL), needs("hook", CORE_HOOK)]
                .iter()
                .chain(entries)
                .map(|line| format!("  - {line}")),
        );
        text.push(String::new());
        write(&self.catalog_dir.join("packs/core.yml"), &text.join("\n"));
    }

    /// Adds a skill to the fixture catalog, its name derived from its path per §2.
    ///
    /// The `requires` graphs under test (a chain, a diamond, a cycle) cannot live in the shared
    /// fixture: a cycle there would fail every other profile, and `validate` is meant to reject
    /// exactly that catalog. So each shape is written into the copy this test owns.
    fn write_skill(&self, relative: &str, annotations: &[String]) {
        write(
            &self
                .catalog_dir
                .join("skills")
                .join(relative)
                .join("SKILL.md"),
            &skill_document(relative, annotations),
        );
    }

    /// Adds a skill to a second catalog beside the fixture, for the cases about two catalogs
    /// providing one name. `catalog` is the catalog's directory, which is also the name config
    /// gives it.
    fn write_skill_in(&self, catalog: &str, relative: &str, annotations: &[String]) {
        write(
            &self
                .root
                .join(catalog)
                .join("skills")
                .join(relative)
                .join("SKILL.md"),
            &skill_document(relative, annotations),
        );
    }

    /// Points the project at the fixture catalog and a second one beside it, with a `requires`
    /// list.
    ///
    /// The second catalog gets an alias of its own, which is what makes both copies of a name
    /// individually addressable, and so what makes the collision below the project's to avoid.
    fn write_two_catalog_profile(&self, second: &str, requires: &[String]) {
        let mut text = vec![
            "version: 1".to_owned(),
            "catalogs:".to_owned(),
            format!("  - name: {CATALOG_NAME}"),
            "    source: path:../catalog".to_owned(),
            format!("  - name: {second}"),
            format!("    source: path:../{second}"),
            "requires:".to_owned(),
        ];

        text.extend(requires.iter().cloned());
        text.push(String::new());
        write(&self.project_dir.join("ambit.yml"), &text.join("\n"));
    }

    /// Adds a hook to the fixture catalog, its name derived from its path per §2.
    fn write_hook(&self, name: &str, hook_lines: &[&str]) {
        let mut text = vec![format!("name: {name}")];

        text.extend(hook_lines.iter().map(|&line| line.to_owned()));
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

    /// What source resolution reads from outside its arguments; every source here is a local
    /// path.
    fn context(&self) -> SourceContext {
        SourceContext {
            project_dir: self.project_dir.clone(),
            env: self.env.clone(),
            offline: false,
        }
    }

    /// Resolves the project as it is on disk, in-process, skipping the CLI.
    fn resolve(&self) -> Result<Bundle> {
        let config = load_project_config(&self.project_dir)?;
        let catalogs = load_catalogs(&config, &self.context(), &mut CatalogLoadOptions::default())?;

        resolve_bundle(&config, &merge_catalogs(&catalogs))
    }

    /// Writes a profile and resolves it.
    fn try_bundle(&self, requires: &[String]) -> Result<Bundle> {
        self.write_profile(requires, &[]);
        self.resolve()
    }

    fn bundle(&self, requires: &[String]) -> Bundle {
        self.try_bundle(requires).expect("the profile resolves")
    }

    /// Runs the CLI against the project, collecting stdout and stderr.
    fn cli(&self, args: &[&str]) -> Out {
        let project = self.project_dir.to_string_lossy().into_owned();
        let mut argv: Vec<&str> = args.to_vec();

        argv.extend(["--project", &project]);

        let result = run_cli(&argv, &self.root, &self.env);

        Out {
            code: result.code,
            stdout: strip(&result.stdout),
            stderr: strip(&result.stderr),
        }
    }
}

fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<String> {
    items.iter().map(|item| name(item).to_owned()).collect()
}

fn skill_names(bundle: &Bundle) -> Vec<String> {
    names(&bundle.skills, |skill| &skill.name)
}

fn mcp_names(bundle: &Bundle) -> Vec<String> {
    names(&bundle.mcps, |mcp| &mcp.name)
}

fn hook_names(bundle: &Bundle) -> Vec<String> {
    names(&bundle.hooks, |hook| &hook.name)
}

/// The names of the hooks a case wrote, in bundle order, with the fixture's own left out.
///
/// The fixture's hooks are reached by the same entries these cases hold, and every case here is
/// about the one document it wrote; the golden bundles are where the fixture's own selection is
/// pinned.
fn written_hooks(bundle: &Bundle) -> Vec<String> {
    hook_names(bundle)
        .into_iter()
        .filter(|name| !FIXTURE_HOOKS.contains(&name.as_str()))
        .collect()
}

fn required_by(kind: ItemKind, name: &str) -> SelectionReason {
    SelectionReason::RequiredBy {
        requirer: BundleItem {
            kind,
            name: name.to_owned(),
        },
    }
}

fn selected_entry(reason: Option<&SelectionReason>) -> &PatternEntry {
    match reason {
        Some(SelectionReason::Selected { entry }) => entry,
        other => panic!("expected a selected reason, got {other:?}"),
    }
}

fn s(text: &str) -> String {
    text.to_owned()
}

// Pure cases: hand-built bundles and selections, no catalog on disk.

fn item(kind: ItemKind, name: &str) -> BundleItem {
    BundleItem {
        kind,
        name: name.to_owned(),
    }
}

fn merged_skill(catalog: &str, name: &str) -> MergedSkill {
    MergedSkill {
        name: name.to_owned(),
        path: format!("skills/{name}"),
        description: None,
        requires: Vec::new(),
        expects: Vec::new(),
        catalog: catalog.to_owned(),
        commit: None,
        catalog_root: PathBuf::from("/catalog"),
    }
}

fn merged_pack(catalog: &str, name: &str) -> MergedPack {
    MergedPack {
        name: name.to_owned(),
        plugin: None,
        description: None,
        requires: Vec::new(),
        catalog: catalog.to_owned(),
        file: format!("packs/{name}.yml"),
    }
}

/// A bundle whose reasons form the chain `pack:core` → `skill:a` → `skill:b`, with nothing else in
/// it, since `is_selected`, `reason_of` and `explain_selection` read the reasons alone.
fn chained_bundle() -> Bundle {
    let mut bundle = Bundle::default();

    bundle.reasons.packs.insert(
        s("core"),
        SelectionReason::Selected {
            entry: PatternEntry {
                kind: ItemKind::Pack,
                pattern: s("core"),
                catalog: Some(s(CATALOG_NAME)),
            },
        },
    );
    bundle
        .reasons
        .skills
        .insert(s("a"), required_by(ItemKind::Pack, "core"));
    bundle
        .reasons
        .skills
        .insert(s("b"), required_by(ItemKind::Skill, "a"));
    bundle
}

#[test]
fn formats_an_item_as_kind_and_name() {
    assert_eq!(format_item(&item(ItemKind::Mcp, "sentry")), "mcp:sentry");
    assert_eq!(
        format_item(&item(ItemKind::Pack, "function.engineering")),
        "pack:function.engineering"
    );
}

#[test]
fn positions_a_requirer_entry_by_file_alone() {
    let requirer = Requirer {
        kind: RequirerKind::Skill,
        catalog: s(CATALOG_NAME),
        name: s("a"),
        requires: Vec::new(),
        file: s("skills/a/SKILL.md"),
    };

    assert_eq!(requirer_position(&requirer), "(skills/a/SKILL.md)");
}

#[test]
fn sends_a_reader_to_a_skill_file_and_a_pack_document() {
    let merged = MergedCatalog {
        catalogs: vec![s(CATALOG_NAME)],
        packs: vec![merged_pack(CATALOG_NAME, "core")],
        skills: vec![merged_skill(CATALOG_NAME, "core")],
        ..MergedCatalog::default()
    };
    let requirers = requirers_of(&merged);

    // Packs first, and a pack and a skill of one name are two requirers.
    assert_eq!(
        requirers
            .iter()
            .map(|requirer| (requirer.kind, requirer.file.as_str()))
            .collect::<Vec<_>>(),
        [
            (RequirerKind::Pack, "packs/core.yml"),
            (RequirerKind::Skill, "skills/core/SKILL.md"),
        ]
    );
}

#[test]
fn walks_a_chain_back_to_the_entry_that_started_it() {
    let bundle = chained_bundle();
    let chain = explain_selection(&bundle, &item(ItemKind::Skill, "b")).expect("explains");

    assert_eq!(
        chain
            .iter()
            .map(|link| format_item(&item(link.kind, &link.name)))
            .collect::<Vec<_>>(),
        ["pack:core", "skill:a", "skill:b"]
    );
    assert_eq!(chain[2].reason, required_by(ItemKind::Skill, "a"));
}

#[test]
fn answers_whether_an_item_is_selected_without_confusing_namespaces() {
    let bundle = chained_bundle();

    assert!(is_selected(&bundle, &item(ItemKind::Pack, "core")));
    assert!(!is_selected(&bundle, &item(ItemKind::Skill, "core")));
}

#[test]
fn refuses_to_explain_an_item_outside_the_bundle_as_a_bug() {
    let bundle = chained_bundle();
    let error = reason_of(&bundle, &item(ItemKind::Hook, "absent")).expect_err("not selected");

    assert_eq!(error.code, ExitCode::Internal);
    assert_eq!(error.message, "cannot explain hook \"absent\"");
    assert_eq!(
        error.detail,
        [
            "it is not in the bundle",
            "this is a bug in ambit; please report it"
        ]
    );
}

#[test]
fn refuses_a_chain_that_does_not_terminate() {
    let mut bundle = Bundle::default();

    bundle
        .reasons
        .skills
        .insert(s("a"), required_by(ItemKind::Skill, "b"));
    bundle
        .reasons
        .skills
        .insert(s("b"), required_by(ItemKind::Skill, "a"));

    let error = explain_selection(&bundle, &item(ItemKind::Skill, "a")).expect_err("loops");

    assert_eq!(error.code, ExitCode::Internal);
    assert_eq!(
        error.detail[0],
        "the `requires` chain through skill:b does not terminate"
    );
}

#[test]
fn refuses_two_copies_of_one_name_naming_every_provider() {
    let selection = Selection {
        skills: vec![
            merged_skill(CATALOG_NAME, "house-style"),
            merged_skill("personal", "house-style"),
        ],
        ..Selection::default()
    };
    let error = assert_no_collisions(&selection).expect_err("collides");

    assert_eq!(error.code, ExitCode::Resolution);
    assert_eq!(
        error.message,
        "skill \"house-style\" is selected from more than one catalog"
    );
    assert_eq!(
        error.detail,
        [
            "provided by: company, personal",
            "a harness reads one entry per name, so both copies would be installed at the same path",
            "select only one copy: narrow a `requires` pattern, or drop the entry that reaches the other catalog",
        ]
    );
}

#[test]
fn refuses_two_selected_packs_of_one_name_with_their_own_reason() {
    let selection = Selection {
        packs: vec![
            merged_pack(CATALOG_NAME, "core"),
            merged_pack("personal", "core"),
        ],
        ..Selection::default()
    };
    let error = assert_no_collisions(&selection).expect_err("collides");

    assert_eq!(
        error.detail[1],
        "a bundle holds one item per name, so there would be two answers to which one is installed"
    );
}

#[test]
fn accepts_one_copy_per_name_in_each_namespace() {
    let selection = Selection {
        packs: vec![merged_pack(CATALOG_NAME, "core")],
        skills: vec![
            merged_skill(CATALOG_NAME, "core"),
            merged_skill("personal", "other"),
        ],
        ..Selection::default()
    };

    assert!(assert_no_collisions(&selection).is_ok());
}

#[test]
fn closes_hand_built_requirers_and_names_the_edge_of_a_cycle() {
    let mut a = merged_skill(CATALOG_NAME, "a");
    let mut b = merged_skill(CATALOG_NAME, "b");
    let unqualified = |name: &str| PatternEntry {
        kind: ItemKind::Skill,
        pattern: s(name),
        catalog: None,
    };

    a.requires = vec![unqualified("b")];
    b.requires = vec![unqualified("a")];

    let merged = MergedCatalog {
        catalogs: vec![s(CATALOG_NAME)],
        skills: vec![a, b],
        ..MergedCatalog::default()
    };
    let roots: Vec<Requirer> = requirers_of(&merged).into_iter().take(1).collect();
    let error = close_over_requires(&roots, &[], &[], &merged).expect_err("cycles");

    assert_eq!(
        error.detail,
        [
            "skill:a → skill:b → skill:a",
            "closed by `skill:a` in skills/b/SKILL.md",
            "break the cycle by removing one `requires` entry",
        ]
    );
}

#[test]
fn says_when_a_project_configures_no_catalogs_at_all() {
    let entry = PatternEntry {
        kind: ItemKind::Skill,
        pattern: s("core"),
        catalog: Some(s("company")),
    };
    let error = unmatched_entry_error(&entry, "company", "(ambit.yml line 6)", &[]);

    assert_eq!(
        error.message,
        "`requires` entry \"skill:company/core\" matches nothing (ambit.yml line 6)"
    );
    assert_eq!(
        error.detail,
        [
            "no catalog in `catalogs:` is named \"company\"",
            "this project configures no catalogs at all",
            "correct the qualifier, or add the catalog to `catalogs:`",
        ]
    );
}

// The glob rules, end to end against a catalog rather than against the matcher alone.
//
// `pattern` pins what a pattern matches; these pin that *selection* runs on it: the exclusion in
// particular, since `core.*` not reaching `core` is the one rule whose cost the design accepts as
// silent, and the only place it can be caught is a case that writes both items.

fn with_prefix_skills() -> Fixture {
    let fixture = Fixture::new();

    fixture.write_skill("prefix", &[]);
    fixture.write_skill("prefix.child", &[]);
    fixture.write_skill("prefix.child.deeper", &[]);
    fixture.write_skill("prefix-sibling", &[]);
    fixture
}

#[test]
fn glob_reaches_every_depth_beneath_a_prefix_since_star_spans_the_dot() {
    let fixture = with_prefix_skills();
    let selected = fixture.bundle(&[entry("skill", "prefix.*")]);

    assert_eq!(
        skill_names(&selected),
        ["prefix.child", "prefix.child.deeper"]
    );
}

#[test]
fn glob_excludes_the_item_named_exactly_the_prefix_which_takes_a_second_entry() {
    let fixture = with_prefix_skills();
    let one = fixture.bundle(&[entry("skill", "prefix.*")]);

    assert!(!skill_names(&one).contains(&s("prefix")));

    let two = fixture.bundle(&[entry("skill", "prefix"), entry("skill", "prefix.*")]);

    assert!(skill_names(&two).contains(&s("prefix")));
}

#[test]
fn glob_does_not_reach_a_sibling_whose_name_merely_starts_with_the_patterns() {
    // `prefix-sibling` reads as a hierarchy to a bare prefix check and to nobody else; the dot in
    // `prefix.*` is a literal character the sibling does not have.
    let fixture = with_prefix_skills();
    let selected = fixture.bundle(&[entry("skill", "prefix.*")]);

    assert!(!skill_names(&selected).contains(&s("prefix-sibling")));
}

#[test]
fn glob_takes_a_whole_namespace_for_a_bare_star_one_entry_per_namespace() {
    // A key names one namespace, so `*` is as wide as an entry gets: taking the whole catalog is
    // four entries, which is the grammar declining to guess how much of it somebody meant.
    let fixture = with_prefix_skills();
    let everything = fixture.bundle(&[entry("skill", "*"), entry("mcp", "*")]);

    assert_eq!(everything.skills.len(), 8);
    assert_eq!(mcp_names(&everything), ["fixture", "linter"]);
    // The packs are a namespace of their own and no `skill: *` reaches them.
    assert_eq!(everything.packs.len(), 0);
}

#[test]
fn glob_matches_an_exact_name_and_nothing_else_when_the_pattern_holds_no_wildcard() {
    let fixture = with_prefix_skills();
    let selected = fixture.bundle(&[entry("skill", "prefix.child")]);

    assert_eq!(skill_names(&selected), ["prefix.child"]);
}

// The two halves of an entry that are declared rather than guessed, and what each of them refuses
// to reach.

#[test]
fn never_reaches_a_pack_from_a_skill_entry_or_the_reverse() {
    // The reason the key is written out at all: a catalog's namespaces are flat and independent,
    // so one name can legitimately belong to a pack and to a skill, and an entry says which it
    // means.
    let fixture = Fixture::new();

    fixture.write_skill("core", &[]);

    assert_eq!(
        skill_names(&fixture.bundle(&[entry("skill", "core")])),
        ["core"]
    );
    assert_eq!(fixture.bundle(&[entry("skill", "core")]).packs.len(), 0);
    assert_eq!(
        skill_names(&fixture.bundle(&[entry("pack", "core")])),
        [CORE_SKILL]
    );
}

#[test]
fn takes_a_pack_whole_which_is_the_grouping_a_project_asked_for() {
    // A pack is not a filter a consumer narrows, it is a set the catalog decided on, so taking
    // `core` takes the hook in it as well as the skill.
    let fixture = Fixture::new();
    let core = fixture.bundle(&[entry("pack", "core")]);

    assert_eq!(skill_names(&core), [CORE_SKILL]);
    assert_eq!(hook_names(&core), [CORE_HOOK]);
}

#[test]
fn confines_an_entry_to_the_catalog_it_qualified() {
    let fixture = Fixture::new();

    fixture.write_skill_in("personal", "personal-only", &[]);
    fixture.write_two_catalog_profile(
        "personal",
        &[entry("pack", &format!("{CATALOG_NAME}/core"))],
    );

    let resolved = fixture.resolve().expect("resolves");

    assert_eq!(skill_names(&resolved), [CORE_SKILL]);
}

// Selection by pattern.

fn engineering() -> Vec<String> {
    vec![
        entry("pack", "function.engineering"),
        entry("pack", "function.engineering.*"),
    ]
}

fn core_and_engineering() -> Vec<String> {
    vec![
        entry("pack", "core"),
        entry("pack", "function.engineering"),
        entry("pack", "function.engineering.*"),
    ]
}

#[test]
fn selects_only_what_an_entry_reaches_nothing_is_implicit() {
    let fixture = Fixture::new();
    let wide = fixture.bundle(&engineering());

    // `company-context` is here because `function.engineering` requires the `core` pack, which is
    // the composition packs exist for, and `acme-brief` is not, because nothing named it.
    assert_eq!(
        skill_names(&wide),
        [ENGINEERING_SKILL, CORE_SKILL, FRONTEND_SKILL]
    );
    assert!(!skill_names(&wide).contains(&s(PROJECT_SKILL)));
}

#[test]
fn selects_the_union_of_every_entry_in_the_list() {
    let fixture = Fixture::new();
    let both = fixture.bundle(&core_and_engineering());

    assert_eq!(
        skill_names(&both),
        [ENGINEERING_SKILL, CORE_SKILL, FRONTEND_SKILL]
    );
}

#[test]
fn does_not_reach_a_pack_no_entry_names() {
    let fixture = Fixture::new();

    for requires in [engineering(), core_and_engineering()] {
        let resolved = fixture.bundle(&requires);

        assert!(!skill_names(&resolved).contains(&s(PROJECT_SKILL)));
    }
}

#[test]
fn reaches_exactly_the_pack_a_narrow_entry_names_and_what_that_pack_requires() {
    let fixture = Fixture::new();
    let frontend = fixture.bundle(&[entry("pack", "function.engineering.frontend")]);

    // Its own skill, plus everything the two packs beneath it name, and nothing from the project
    // pack, which nothing here reaches.
    assert_eq!(
        skill_names(&frontend),
        [ENGINEERING_SKILL, CORE_SKILL, FRONTEND_SKILL]
    );
    assert!(!skill_names(&frontend).contains(&s(PROJECT_SKILL)));
}

#[test]
fn yields_an_empty_bundle_for_an_empty_requires_list() {
    let fixture = Fixture::new();

    assert_eq!(fixture.bundle(&[]), Bundle::default());
}

#[test]
fn selects_an_mcp_server_through_the_pack_that_names_it() {
    let fixture = Fixture::new();

    assert_eq!(mcp_names(&fixture.bundle(&engineering())), ["linter"]);
    assert_eq!(fixture.bundle(&[entry("pack", "core")]).mcps.len(), 0);
}

#[test]
fn unions_expects_across_everything_the_list_selected() {
    // ACME_FIGMA_TOKEN comes from the nested frontend skill, LINTER_API_KEY from the server the
    // broader entry selects, so one list must produce both.
    let fixture = Fixture::new();

    assert_eq!(
        fixture.bundle(&engineering()).expects.env,
        ["ACME_FIGMA_TOKEN", "LINTER_API_KEY"]
    );
}

#[test]
fn selects_an_item_once_when_two_entries_both_reach_it() {
    // The pack and the exact name both reach `company-context`, and it is one item either way: a
    // bundle holds one entry per name, and which entry is reported is the reason's business.
    let fixture = Fixture::new();
    let twice = fixture.bundle(&[entry("pack", "core"), entry("skill", CORE_SKILL)]);

    assert_eq!(skill_names(&twice), [CORE_SKILL]);
}

// Spec §4.9: the closure is what makes a skill's dependencies travel with it. The fixture's project
// skill is the case the spec cares about (it requires a skill and a server that no entry selecting
// the project skill itself would reach), and the graph shapes around it (chain, diamond, cycle)
// are written into the catalog per test.

#[test]
fn closure_pulls_in_a_required_skill_and_mcp_server_no_entry_matches() {
    let fixture = Fixture::new();
    let project = fixture.bundle(&[entry("pack", "project.acme")]);

    assert_eq!(skill_names(&project), [PROJECT_SKILL, CORE_SKILL]);
    assert_eq!(mcp_names(&project), ["fixture"]);
}

#[test]
fn closure_unions_expects_over_what_it_added() {
    // FIXTURE_API_KEY belongs to the server only `requires` can reach, so a bundle that lists the
    // server without its credential would send `doctor` looking at the wrong thing.
    let fixture = Fixture::new();

    assert_eq!(
        fixture.bundle(&[entry("pack", "project.acme")]).expects.env,
        ["FIXTURE_API_KEY"]
    );
}

#[test]
fn closure_follows_a_requirement_of_a_requirement_to_fixpoint() {
    let fixture = Fixture::new();

    fixture.write_skill("chain-a", &[requires(&[needs("skill", "chain-b")])]);
    fixture.write_skill("chain-b", &[requires(&[needs("skill", "chain-c")])]);
    fixture.write_skill("chain-c", &[]);
    fixture.in_core_pack(&[needs("skill", "chain-a")]);

    assert_eq!(
        skill_names(&fixture.bundle(&[entry("pack", "core")])),
        ["chain-a", "chain-b", "chain-c", CORE_SKILL]
    );
}

#[test]
fn closure_treats_a_shared_requirement_as_a_diamond_not_a_cycle() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "diamond-left",
        &[requires(&[needs("skill", "diamond-shared")])],
    );
    fixture.write_skill(
        "diamond-right",
        &[requires(&[needs("skill", "diamond-shared")])],
    );
    fixture.write_skill("diamond-shared", &[]);
    fixture.in_core_pack(&[
        needs("skill", "diamond-left"),
        needs("skill", "diamond-right"),
    ]);

    let resolved = fixture.bundle(&[entry("pack", "core")]);

    assert_eq!(
        skill_names(&resolved)
            .into_iter()
            .filter(|name| name.starts_with("diamond-"))
            .collect::<Vec<_>>(),
        ["diamond-left", "diamond-right", "diamond-shared"]
    );
}

#[test]
fn closure_selects_a_required_item_exactly_once_however_many_require_it() {
    let fixture = Fixture::new();

    fixture.write_skill("twice-left", &[requires(&[needs("mcp", "fixture")])]);
    fixture.write_skill("twice-right", &[requires(&[needs("mcp", "fixture")])]);
    fixture.in_core_pack(&[needs("skill", "twice-left"), needs("skill", "twice-right")]);

    assert_eq!(
        mcp_names(&fixture.bundle(&[entry("pack", "core")])),
        ["fixture"]
    );
}

#[test]
fn closure_takes_the_requiring_catalogs_copy_of_a_name_two_catalogs_ship() {
    // A name two catalogs ship is not ambiguous to a requirer, because the entry never leaves its
    // own catalog: `company`'s skill gets `company`'s copy and `personal`'s is not selected at all.
    // So the closure cannot pull one name in twice, and there is no collision here to refuse.
    let fixture = Fixture::new();

    fixture.write_skill("needs-shared", &[requires(&[needs("skill", "shared-dep")])]);
    fixture.write_skill("shared-dep", &[]);
    fixture.in_core_pack(&[needs("skill", "needs-shared")]);
    fixture.write_skill_in("personal", "shared-dep", &[]);
    fixture.write_two_catalog_profile(
        "personal",
        &[entry("pack", &format!("{CATALOG_NAME}/core"))],
    );

    let resolved = fixture.resolve().expect("resolves");

    assert_eq!(
        resolved
            .skills
            .iter()
            .filter(|skill| skill.name == "shared-dep")
            .map(|skill| skill.catalog.as_str())
            .collect::<Vec<_>>(),
        [CATALOG_NAME]
    );
}

#[test]
fn closure_refuses_a_collision_two_project_entries_reach() {
    // Collision is a project's ask, not a catalog's: both catalogs ship `house-style`, both entries
    // select a copy, and the two would materialize to one harness path.
    let fixture = Fixture::new();

    fixture.write_skill("house-style", &[]);
    fixture.write_skill_in("personal", "house-style", &[]);
    fixture.write_two_catalog_profile(
        "personal",
        &[
            entry("skill", &format!("{CATALOG_NAME}/house-style")),
            entry("skill", "personal/house-style"),
        ],
    );

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("skill \"house-style\" is selected from more than one catalog")
    );
    assert!(
        result
            .stderr
            .contains(&format!("provided by: {CATALOG_NAME}, personal"))
    );
}

#[test]
fn closure_leaves_a_broken_skill_nobody_selected_alone() {
    // Spec §4's validation split: `resolve` hard-validates the selected closure only. No entry
    // names this skill, so nothing reaches it and its dangling requirement is `validate`'s
    // business, not this bundle's.
    let fixture = Fixture::new();

    fixture.write_skill(
        "broken-unselected",
        &[requires(&[needs("skill", "absent-skill")])],
    );

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
}

// A `requires` entry inside a catalog that reaches nothing, which is the one refusal a `requires`
// list earns at either altitude: the same finding a project's own entry earns, named the same way.

#[test]
fn catalog_entry_exits_3_naming_the_entry_the_catalog_and_the_file() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "broken-dangling",
        &[requires(&[needs("skill", "absent-skill")])],
    );
    fixture.in_core_pack(&[needs("skill", "broken-dangling")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stderr.contains(
        "`requires` entry \"skill:absent-skill\" matches nothing (skills/broken-dangling/SKILL.md)"
    ));
    assert!(result.stderr.contains(&format!(
        "no skill in catalog \"{CATALOG_NAME}\" has a name matching \"absent-skill\""
    )));
}

#[test]
fn catalog_entry_names_the_mcp_namespace_and_only_it() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "broken-dangling-mcp",
        &[requires(&[needs("mcp", "absent")])],
    );
    fixture.in_core_pack(&[needs("skill", "broken-dangling-mcp")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("`requires` entry \"mcp:absent\" matches nothing")
    );
    assert!(
        result
            .stderr
            .contains(&format!("no MCP server in catalog \"{CATALOG_NAME}\""))
    );
}

#[test]
fn catalog_entry_names_the_hook_namespace_and_only_it() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "broken-dangling-hook",
        &[requires(&[needs("hook", "absent")])],
    );
    fixture.in_core_pack(&[needs("skill", "broken-dangling-hook")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stderr.contains(
        "`requires` entry \"hook:absent\" matches nothing (skills/broken-dangling-hook/SKILL.md)"
    ));
    assert!(
        result
            .stderr
            .contains(&format!("no hook in catalog \"{CATALOG_NAME}\""))
    );
}

#[test]
fn catalog_entry_naming_hooks_does_not_accept_a_skill_of_that_name() {
    // A `hook:` entry names the hook namespace, and a skill called `absent` is not in it, or the
    // key would be decoration.
    let fixture = Fixture::new();

    fixture.write_skill("absent", &[]);
    fixture.write_skill(
        "broken-wrong-namespace",
        &[requires(&[needs("hook", "absent")])],
    );
    fixture.in_core_pack(&[
        needs("skill", "absent"),
        needs("skill", "broken-wrong-namespace"),
    ]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("`requires` entry \"hook:absent\" matches nothing")
    );
}

#[test]
fn catalog_entry_is_not_satisfied_by_another_catalogs_copy() {
    // The merged view plainly holds `remote-only`, and `company`'s skill still cannot require it. A
    // catalog author cannot write a consumer's alias, so a bare pattern means this catalog.
    let fixture = Fixture::new();

    fixture.write_skill(
        "needs-across",
        &[requires(&[needs("skill", "remote-only")])],
    );
    fixture.in_core_pack(&[needs("skill", "needs-across")]);
    fixture.write_skill_in("personal", "remote-only", &[]);
    fixture.write_two_catalog_profile(
        "personal",
        &[entry("pack", &format!("{CATALOG_NAME}/core"))],
    );

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("`requires` entry \"skill:remote-only\" matches nothing")
    );
    assert!(result.stderr.contains(
        "a catalog's own `requires` resolves within that catalog, which can only require what it ships"
    ));
}

#[test]
fn catalog_entry_takes_a_wildcard_reaching_every_sibling_under_a_prefix() {
    // The point of one grammar at both altitudes: a skill can say *everything under `dep.`* exactly
    // as a project can, and it is one entry rather than one per sibling.
    let fixture = Fixture::new();

    fixture.write_skill("wide", &[requires(&[needs("skill", "dep.*")])]);
    fixture.write_skill("dep.one", &[]);
    fixture.write_skill("dep.two", &[]);
    fixture.in_core_pack(&[needs("skill", "wide")]);

    assert_eq!(
        skill_names(&fixture.bundle(&[entry("pack", "core")])),
        [CORE_SKILL, "dep.one", "dep.two", "wide"]
    );
}

#[test]
fn catalog_entry_wildcard_is_bounded_by_its_namespace() {
    let fixture = Fixture::new();

    fixture.write_hook(
        "guard-tagged",
        &[
            "event: PreToolUse",
            "matcher: Bash",
            "type: command",
            "command: npx guard",
        ],
    );
    fixture.write_skill("guarded", &[requires(&[needs("hook", "*")])]);
    fixture.in_core_pack(&[needs("skill", "guarded")]);

    let required = fixture.bundle(&[entry("pack", "core")]);

    // Every hook the catalog ships, and no skill or server: the namespace is what bounds a
    // wildcard.
    assert_eq!(
        hook_names(&required),
        ["acme-standup", "guard-secrets", "guard-tagged", CORE_HOOK]
    );
    assert_eq!(required.mcps.len(), 0);
}

#[test]
fn catalog_entry_written_as_a_bare_string_is_refused() {
    // A plain list of names is the shape a reader reaches for, and it says nothing about which of
    // the four namespaces it means: the one declaration this grammar is made of.
    let fixture = Fixture::new();

    fixture.write_skill("legacy", &[s("requires: [mcp.absent]")]);
    fixture.in_core_pack(&[needs("skill", "legacy")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains("`requires` entry \"mcp.absent\" is not a mapping")
    );
    assert!(
        result
            .stderr
            .contains("does not say which namespace it selects from")
    );
    assert!(result.stderr.contains("- skill: \"mcp.absent\""));
}

#[test]
fn catalog_entry_reads_a_one_key_entry_naming_a_namespace() {
    let fixture = Fixture::new();

    fixture.write_skill("modern", &[s("requires: [{ mcp: fixture }]")]);
    fixture.in_core_pack(&[needs("skill", "modern")]);

    assert_eq!(
        mcp_names(&fixture.bundle(&[entry("pack", "core")])),
        ["fixture"]
    );
}

#[test]
fn catalog_entry_that_writes_a_qualifier_is_refused() {
    // A catalog author cannot write the alias, so an address that carries one is exit 2 rather than
    // a pattern quietly resolved against a guess.
    let fixture = Fixture::new();

    fixture.write_skill(
        "qualified",
        &[format!(
            "requires: [{{ skill: \"{CATALOG_NAME}/{CORE_SKILL}\" }}]"
        )],
    );
    fixture.in_core_pack(&[needs("skill", "qualified")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains(&format!(
        "`requires` entry \"{CATALOG_NAME}/{CORE_SKILL}\" names a catalog, which a catalog's own `requires` may not"
    )));
}

#[test]
fn catalog_entry_whose_key_names_no_namespace_is_refused() {
    let fixture = Fixture::new();

    fixture.write_skill("typo", &[s("requires: [{ skil: a }]")]);
    fixture.in_core_pack(&[needs("skill", "typo")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains("unknown key \"ambit.requires[0].skil\"")
    );
    assert!(
        result
            .stderr
            .contains("accepted keys: hook, mcp, pack, skill")
    );
}

// `expects` is the last list written as one-key `<kind>: <name>` mappings, so the shape errors
// here are what that grammar refuses.
//
// The one thing deliberately *not* here is a resolution failure: an expectation names nothing a
// catalog could provide, so there is no entry to write that could reach nothing and no cycle to
// close. That is `doctor`'s question.

#[test]
fn expects_entry_written_as_a_bare_string_is_refused() {
    let fixture = Fixture::new();

    fixture.write_skill("legacy", &[s("expects: [CLOSE_API_KEY]")]);
    fixture.in_core_pack(&[needs("skill", "legacy")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains("`expects` entry \"CLOSE_API_KEY\" names no precondition")
    );
    assert!(result.stderr.contains("`- env: CLOSE_API_KEY`"));
}

#[test]
fn expects_entry_naming_two_preconditions_is_refused() {
    let fixture = Fixture::new();

    fixture.write_skill("greedy", &[s("expects: [{env: A, bin: b}]")]);
    fixture.in_core_pack(&[needs("skill", "greedy")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains("an `expects` entry names 2 preconditions: bin, env")
    );
}

#[test]
fn expects_kind_this_version_does_not_know_is_refused() {
    // `bin:` is the obvious second kind and is deliberately not one yet, so this doubles as the
    // claim that a catalog written against a later ambit fails loudly here rather than silently
    // declaring nothing.
    let fixture = Fixture::new();

    fixture.write_skill("ahead", &[s("expects: [{bin: docker}]")]);
    fixture.in_core_pack(&[needs("skill", "ahead")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains("unknown precondition \"bin\" in an `expects` entry")
    );
}

#[test]
fn expects_is_taken_on_all_three_kinds() {
    let fixture = Fixture::new();

    fixture.write_skill("reader", &[s("expects: [{env: SKILL_VAR}]")]);
    fixture.write_mcp("server", &["expects: [{env: MCP_VAR}]"]);
    fixture.write_hook(
        "watcher",
        &[
            "event: Stop",
            "type: command",
            "command: npx watch",
            "expects: [{env: HOOK_VAR}]",
        ],
    );
    fixture.in_core_pack(&[
        needs("skill", "reader"),
        needs("mcp", "server"),
        needs("hook", "watcher"),
    ]);

    let env = fixture.bundle(&[entry("pack", "core")]).expects.env;

    for variable in ["HOOK_VAR", "MCP_VAR", "SKILL_VAR"] {
        assert!(env.contains(&s(variable)), "{variable} in {env:?}");
    }
}

// Requirement cycles.

#[test]
fn cycle_exits_3_printing_the_whole_path() {
    let fixture = Fixture::new();

    fixture.write_skill("cycle-a", &[requires(&[needs("skill", "cycle-b")])]);
    fixture.write_skill("cycle-b", &[requires(&[needs("skill", "cycle-c")])]);
    fixture.write_skill("cycle-c", &[requires(&[needs("skill", "cycle-a")])]);
    fixture.in_core_pack(&[needs("skill", "cycle-a")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stderr.contains("requirement cycle"));
    assert!(
        result
            .stderr
            .contains("skill:cycle-a → skill:cycle-b → skill:cycle-c → skill:cycle-a")
    );
    // The closing edge, which is the actionable half: the entry, and the file it is written in.
    assert!(
        result
            .stderr
            .contains("closed by `skill:cycle-a` in skills/cycle-c/SKILL.md")
    );
    assert!(
        result
            .stderr
            .contains("break the cycle by removing one `requires` entry")
    );
}

#[test]
fn cycle_of_one_skill_requiring_itself_is_reported() {
    let fixture = Fixture::new();

    fixture.write_skill("cycle-self", &[requires(&[needs("skill", "cycle-self")])]);
    fixture.in_core_pack(&[needs("skill", "cycle-self")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("skill:cycle-self → skill:cycle-self")
    );
}

#[test]
fn cycle_reached_only_through_a_requirement_is_reported() {
    let fixture = Fixture::new();

    fixture.write_skill("cycle-entry", &[requires(&[needs("skill", "cycle-b")])]);
    fixture.write_skill("cycle-b", &[requires(&[needs("skill", "cycle-c")])]);
    fixture.write_skill("cycle-c", &[requires(&[needs("skill", "cycle-b")])]);
    fixture.in_core_pack(&[needs("skill", "cycle-entry")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("skill:cycle-b → skill:cycle-c → skill:cycle-b")
    );
}

#[test]
fn cycle_is_named_the_same_whatever_order_a_requires_list_is_written_in() {
    let fixture = Fixture::new();

    fixture.write_skill(
        "cycle-a",
        &[requires(&[
            needs("skill", "cycle-b"),
            needs("skill", "cycle-c"),
        ])],
    );
    fixture.write_skill("cycle-b", &[requires(&[needs("skill", "cycle-a")])]);
    fixture.write_skill("cycle-c", &[requires(&[needs("skill", "cycle-a")])]);
    fixture.in_core_pack(&[needs("skill", "cycle-a")]);
    let first = fixture.cli(&["resolve"]);

    fixture.write_skill(
        "cycle-a",
        &[requires(&[
            needs("skill", "cycle-c"),
            needs("skill", "cycle-b"),
        ])],
    );
    fixture.in_core_pack(&[needs("skill", "cycle-a")]);
    let second = fixture.cli(&["resolve"]);

    assert!(
        first
            .stderr
            .contains("skill:cycle-a → skill:cycle-b → skill:cycle-a")
    );
    assert_eq!(second.stderr, first.stderr);
}

// An exact name is a pattern with no wildcard. The keys that used to do the selecting, and the two
// forms that let a project define an item of its own, are all refused. The refusals are asserted
// end to end here because the exit code is what a user meets; their wording is `config`'s.

#[test]
fn exact_name_selects_a_name_from_a_catalog() {
    let fixture = Fixture::new();
    let named = fixture.bundle(&[entry("skill", ENGINEERING_SKILL)]);

    assert_eq!(skill_names(&named), [ENGINEERING_SKILL]);
    assert_eq!(
        selected_entry(named.reasons.skills.get(ENGINEERING_SKILL)).kind,
        ItemKind::Skill
    );
}

#[test]
fn exact_name_closes_a_named_skill_over_its_own_requires() {
    let fixture = Fixture::new();
    let named = fixture.bundle(&[entry("skill", PROJECT_SKILL)]);

    assert_eq!(skill_names(&named), [PROJECT_SKILL, CORE_SKILL]);
    assert_eq!(mcp_names(&named), ["fixture"]);
    assert_eq!(named.expects.env, ["FIXTURE_API_KEY"]);
}

#[test]
fn exact_name_reaches_an_mcp_server_and_a_hook() {
    let fixture = Fixture::new();
    let named = fixture.bundle(&[entry("mcp", "fixture"), entry("hook", "acme-standup")]);

    assert_eq!(mcp_names(&named), ["fixture"]);
    assert_eq!(hook_names(&named), ["acme-standup"]);
    assert_eq!(named.skills.len(), 0);
}

#[test]
fn exact_name_no_catalog_provides_exits_3_naming_the_entry_and_its_line() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("skill", "absent-skill")], &[]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stderr.contains(&format!(
        "`requires` entry \"skill:{CATALOG_NAME}/absent-skill\" matches nothing (ambit.yml line {FIRST_ENTRY_LINE})"
    )));
    assert!(result.stderr.contains(&format!(
        "no skill in catalog \"{CATALOG_NAME}\" has a name matching \"absent-skill\""
    )));
    assert!(
        result
            .stderr
            .contains("correct the pattern, add the item to a catalog")
    );
}

#[test]
fn exits_2_for_a_top_level_scopes() {
    let fixture = Fixture::new();

    fixture.write_profile(&[], &["scopes:", "  - core"]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("top-level `scopes` is gone"));
    assert!(
        result.stderr.contains(
            "declare a pack in the catalog that requires them, and select it with `pack:`"
        )
    );
}

#[test]
fn exits_2_for_a_top_level_skills() {
    let fixture = Fixture::new();

    fixture.write_profile(&[], &["skills:", &format!("  - {CORE_SKILL}")]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("top-level `skills` is gone"));
    assert!(result.stderr.contains(&format!(
        "`{CORE_SKILL}` becomes `- skill: \"{CATALOG_NAME}/{CORE_SKILL}\"`"
    )));
}

#[test]
fn exits_2_for_a_top_level_mcps() {
    let fixture = Fixture::new();

    fixture.write_profile(
        &[],
        &[
            "mcps:",
            "  - name: custom",
            "    transport:",
            "      stdio:",
            "        command: custom-mcp",
        ],
    );

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("top-level `mcps` is gone"));
    assert!(
        result
            .stderr
            .contains("move each entry to `mcps/<name>.yml`")
    );
    assert!(
        result
            .stderr
            .contains("`- name: local` with `source: path:.`")
    );
}

#[test]
fn exits_2_for_a_top_level_hooks() {
    let fixture = Fixture::new();

    fixture.write_profile(
        &[],
        &[
            "hooks:",
            "  - name: notify",
            "    event: Stop",
            "    type: command",
            "    command: ./notify",
        ],
    );

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("top-level `hooks` is gone"));
    assert!(
        result
            .stderr
            .contains("move each entry to `hooks/<name>/hook.yml`")
    );
}

// A hook a catalog provides is selected exactly as a server is: by a `hook:` entry whose pattern
// reaches it, or by a pack that names it, and belonging to no pack leaves it reachable by name or
// by a `requires` edge alone.

const BLOCK_HOOK: &str = "block-rm";

fn with_block_hook() -> Fixture {
    let fixture = Fixture::new();

    fixture.write_hook(
        BLOCK_HOOK,
        &[
            "event: PreToolUse",
            "matcher: Bash",
            "type: command",
            "command: npx block-rm",
        ],
    );
    fixture
}

#[test]
fn catalog_hook_is_selected_through_a_pack_naming_the_entry_that_reached_it() {
    let fixture = with_block_hook();

    fixture.write_pack("guards", &[needs("hook", BLOCK_HOOK)]);

    let guarded = fixture.bundle(&[entry("pack", "guards")]);

    assert_eq!(written_hooks(&guarded), [BLOCK_HOOK]);
    assert_eq!(guarded.hooks[0].catalog, CATALOG_NAME);
    assert_eq!(
        guarded.hooks[0].r#type,
        crate::model::hook_entity::HookType::Command
    );
    // The reason names the pack the project asked for, not the hook's membership in it: the entry
    // is the half a reader can go and edit.
    assert_eq!(
        guarded.reasons.hooks.get(BLOCK_HOOK),
        Some(&required_by(ItemKind::Pack, "guards"))
    );
}

#[test]
fn catalog_hook_names_the_projects_own_entry_when_one_reaches_it_directly() {
    let fixture = with_block_hook();
    let named = fixture.bundle(&[entry("hook", BLOCK_HOOK)]);

    assert_eq!(
        named.reasons.hooks.get(BLOCK_HOOK),
        Some(&SelectionReason::Selected {
            entry: PatternEntry {
                kind: ItemKind::Hook,
                pattern: s(BLOCK_HOOK),
                catalog: Some(s(CATALOG_NAME)),
            },
        })
    );
}

#[test]
fn catalog_hook_is_reached_through_a_wildcard_entry() {
    let fixture = with_block_hook();
    let wide = fixture.bundle(&[entry("hook", "block-*")]);

    assert_eq!(
        selected_entry(wide.reasons.hooks.get(BLOCK_HOOK)).pattern,
        "block-*"
    );

    let elsewhere = fixture.bundle(&[entry("pack", "core")]);

    assert_eq!(written_hooks(&elsewhere).len(), 0);
}

#[test]
fn catalog_hook_no_pack_names_is_left_out_of_every_pack_selected_bundle() {
    let fixture = with_block_hook();
    let everything = fixture.bundle(&[
        entry("pack", "core"),
        entry("pack", "function.engineering"),
        entry("pack", "function.engineering.*"),
        entry("pack", "project.acme"),
    ]);

    assert_eq!(written_hooks(&everything).len(), 0);
}

#[test]
fn catalog_hook_names_the_catalog_it_came_from_in_resolve() {
    let fixture = with_block_hook();

    fixture.write_profile(
        &[
            entry("hook", BLOCK_HOOK),
            entry("pack", "function.engineering"),
        ],
        &[],
    );

    // Beside the fixture's own hook, which is what the section's padding widens to.
    let width = ENGINEERING_HOOK.len();

    assert!(fixture.cli(&["resolve"]).stdout.contains(&format!(
        "hooks (3)\n  {}  {CATALOG_NAME}  PreToolUse\n  {ENGINEERING_HOOK}  {CATALOG_NAME}  PreToolUse",
        pad_end(BLOCK_HOOK, width)
    )));
}

// A `hook:<name>` requirement is the second route into a bundle, and the only one that reaches a
// hook the project never named: a skill whose instructions are unsafe without its guard carries
// the guard.

const GUARD_HOOK: &str = "guard";

fn with_guard_hook() -> Fixture {
    let fixture = Fixture::new();

    fixture.write_hook(
        GUARD_HOOK,
        &[
            "event: PreToolUse",
            "matcher: Bash",
            "type: command",
            "command: npx guard",
            "expects: [{ env: GUARD_TOKEN }]",
        ],
    );
    fixture
}

#[test]
fn required_hook_is_pulled_in_behind_the_skill_naming_the_requirer() {
    let fixture = with_guard_hook();

    fixture.write_skill("risky", &[requires(&[needs("hook", GUARD_HOOK)])]);
    fixture.in_core_pack(&[needs("skill", "risky")]);

    let required = fixture.bundle(&[entry("pack", "core")]);

    assert_eq!(written_hooks(&required), [GUARD_HOOK]);
    assert_eq!(
        required.reasons.hooks.get(GUARD_HOOK),
        Some(&required_by(ItemKind::Skill, "risky"))
    );
}

#[test]
fn required_hook_expects_feed_the_credential_list() {
    let fixture = with_guard_hook();

    fixture.write_skill("risky", &[requires(&[needs("hook", GUARD_HOOK)])]);
    fixture.in_core_pack(&[needs("skill", "risky")]);

    assert!(
        fixture
            .bundle(&[entry("pack", "core")])
            .expects
            .env
            .contains(&s("GUARD_TOKEN"))
    );
}

#[test]
fn required_hook_is_reached_down_a_chain() {
    let fixture = with_guard_hook();

    fixture.write_skill("chain-leaf", &[requires(&[needs("hook", GUARD_HOOK)])]);
    fixture.write_skill("chain-root", &[requires(&[needs("skill", "chain-leaf")])]);
    fixture.in_core_pack(&[needs("skill", "chain-root")]);

    let required = fixture.bundle(&[entry("pack", "core")]);

    assert_eq!(written_hooks(&required), [GUARD_HOOK]);
    assert_eq!(
        required.reasons.hooks.get(GUARD_HOOK),
        Some(&required_by(ItemKind::Skill, "chain-leaf"))
    );
}

#[test]
fn required_hook_is_left_out_when_nothing_selected_requires_it() {
    // The same catalog, the same hook: what differs is that the requiring skill is not selected,
    // so the edge exists and reaches nothing.
    let fixture = with_guard_hook();

    fixture.write_skill("risky", &[requires(&[needs("hook", GUARD_HOOK)])]);

    assert_eq!(
        written_hooks(&fixture.bundle(&[entry("pack", "core")])).len(),
        0
    );
}

// Spec §6: every selected item carries the reason it is in the bundle, one of the two routes
// resolution offers, and only one. Asserted on the bundle rather than only through `--explain`,
// because the lock records it too and both surfaces have to agree by construction.

#[test]
fn reason_names_the_entry_that_selected_an_item_not_the_value_it_matched() {
    let fixture = Fixture::new();
    let wide = fixture.bundle(&engineering());

    // The reason is the pack the *project* named, not the item's membership in it: a reader
    // looking for why goes to their own `requires` list, which is the half they can change.
    assert_eq!(
        wide.reasons.skills.get(ENGINEERING_SKILL),
        Some(&required_by(ItemKind::Pack, "function.engineering"))
    );
    assert_eq!(
        wide.reasons.packs.get("function.engineering"),
        Some(&SelectionReason::Selected {
            entry: PatternEntry {
                kind: ItemKind::Pack,
                pattern: s("function.engineering"),
                catalog: Some(s(CATALOG_NAME)),
            },
        })
    );
    // Reached through the wildcard entry's pack, and it is that pack the reason names.
    assert_eq!(
        wide.reasons.skills.get(FRONTEND_SKILL),
        Some(&required_by(ItemKind::Pack, FRONTEND_PACK))
    );
    assert_eq!(
        selected_entry(wide.reasons.packs.get(FRONTEND_PACK)).pattern,
        "function.engineering.*"
    );
}

#[test]
fn reason_names_the_requirer_of_a_skill_and_a_server_no_entry_selected() {
    let fixture = Fixture::new();
    let project = fixture.bundle(&[entry("pack", "project.acme")]);

    assert_eq!(
        project.reasons.skills.get(CORE_SKILL),
        Some(&required_by(ItemKind::Skill, PROJECT_SKILL))
    );
    assert_eq!(
        project.reasons.mcps.get("fixture"),
        Some(&required_by(ItemKind::Skill, PROJECT_SKILL))
    );
}

#[test]
fn reason_names_the_first_requirer_by_name_not_by_walk_order() {
    let fixture = Fixture::new();

    fixture.write_skill("twice-left", &[requires(&[needs("mcp", "fixture")])]);
    fixture.write_skill("twice-right", &[requires(&[needs("mcp", "fixture")])]);
    fixture.in_core_pack(&[needs("skill", "twice-left"), needs("skill", "twice-right")]);

    assert_eq!(
        fixture
            .bundle(&[entry("pack", "core")])
            .reasons
            .mcps
            .get("fixture"),
        Some(&required_by(ItemKind::Skill, "twice-left"))
    );
}

#[test]
fn reason_tie_breaks_two_entries_on_sorted_order() {
    // Both routes are true, and an entry beats an edge: the entry ends the chain where the pack's
    // membership continues one.
    let fixture = Fixture::new();
    let both = fixture.bundle(&[
        entry("pack", "function.engineering"),
        entry("skill", ENGINEERING_SKILL),
    ]);
    let chosen = selected_entry(both.reasons.skills.get(ENGINEERING_SKILL));

    assert_eq!(chosen.kind, ItemKind::Skill);
    assert_eq!(chosen.pattern, ENGINEERING_SKILL);
}

#[test]
fn reason_prefers_an_entry_over_a_requires_edge() {
    // `company-context` is both required by the project skill and named outright; the entry is
    // the shorter true answer, and the one the reader can act on.
    let fixture = Fixture::new();
    let both = fixture.bundle(&[entry("pack", "project.acme"), entry("skill", CORE_SKILL)]);

    assert!(matches!(
        both.reasons.skills.get(CORE_SKILL),
        Some(SelectionReason::Selected { .. })
    ));
}

#[test]
fn reason_accounts_for_every_item_it_selected() {
    let fixture = Fixture::new();
    let wide = fixture.bundle(&[
        entry("pack", "core"),
        entry("pack", "function.engineering"),
        entry("pack", "function.engineering.*"),
        entry("pack", "project.acme"),
    ]);

    assert_eq!(
        wide.reasons.skills.keys().cloned().collect::<Vec<_>>(),
        skill_names(&wide)
    );
    assert_eq!(
        wide.reasons.mcps.keys().cloned().collect::<Vec<_>>(),
        mcp_names(&wide)
    );
}

// `ambit resolve --explain`.

#[test]
fn explain_adds_a_reason_column_to_every_section_but_expects() {
    let fixture = Fixture::new();

    fixture.write_profile(&core_and_engineering(), &[]);

    let result = fixture.cli(&["resolve", "--explain"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    let pack = FRONTEND_PACK.len();
    let skill = CORE_SKILL.len();

    assert_eq!(
        result.stdout,
        [
            s("packs (3)"),
            format!(
                "  {}  {CATALOG_NAME}  pack:{CATALOG_NAME}/core",
                pad_end("core", pack)
            ),
            format!(
                "  {}  {CATALOG_NAME}  pack:{CATALOG_NAME}/function.engineering",
                pad_end("function.engineering", pack)
            ),
            format!(
                "  {FRONTEND_PACK}  {CATALOG_NAME}  pack:{CATALOG_NAME}/function.engineering.*"
            ),
            s(""),
            s("skills (3)"),
            format!(
                "  {}  {CATALOG_NAME}  required-by:pack:function.engineering",
                pad_end(ENGINEERING_SKILL, skill)
            ),
            format!("  {CORE_SKILL}  {CATALOG_NAME}  required-by:pack:core"),
            format!(
                "  {}  {CATALOG_NAME}  required-by:pack:function.engineering.frontend",
                pad_end(FRONTEND_SKILL, skill)
            ),
            s(""),
            s("mcps (1)"),
            format!("  linter  {CATALOG_NAME}  required-by:pack:function.engineering"),
            s(""),
            s("hooks (2)"),
            format!(
                "  {ENGINEERING_HOOK}  {CATALOG_NAME}  PreToolUse    required-by:pack:function.engineering"
            ),
            format!("  {CORE_HOOK}  {CATALOG_NAME}  SessionStart  required-by:pack:core"),
            s(""),
            s("expects (2)"),
            s("  env  ACME_FIGMA_TOKEN"),
            s("  env  LINTER_API_KEY"),
        ]
        .join("\n")
    );
}

#[test]
fn explain_adds_a_reason_to_every_json_record_which_plain_json_omits() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "project.acme")], &[]);

    let explained: Value =
        serde_json::from_str(&fixture.cli(&["resolve", "--explain", "--json"]).stdout)
            .expect("JSON");

    assert_eq!(
        explained["skills"][CORE_SKILL]["reason"],
        format!("required-by:skill:{PROJECT_SKILL}")
    );
    assert_eq!(
        explained["skills"][PROJECT_SKILL]["reason"],
        "required-by:pack:project.acme"
    );
    assert_eq!(
        explained["mcps"]["fixture"]["reason"],
        format!("required-by:skill:{PROJECT_SKILL}")
    );

    let plain: Value =
        serde_json::from_str(&fixture.cli(&["resolve", "--json"]).stdout).expect("JSON");

    assert!(plain["skills"][PROJECT_SKILL].get("reason").is_none());
}

// Spec §6: `ambit why <name>` prints the chain from a `requires` entry to the item. The chain
// matters more than the reason (`required-by:x` only moves the question up a level), so the
// assertions are on the whole path, not on the last link.

#[test]
fn why_prints_the_chain_of_something_a_pack_selected() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "core")], &[]);

    let result = fixture.cli(&["why", &format!("skill:{CORE_SKILL}")]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        result.stdout,
        [
            format!("skill {CORE_SKILL}"),
            s(""),
            s("chain (2)"),
            format!(
                "  {}  pack   pack:{CATALOG_NAME}/core",
                pad_end("core", CORE_SKILL.len())
            ),
            format!("  {CORE_SKILL}  skill  required-by:pack:core"),
        ]
        .join("\n")
    );
}

#[test]
fn why_walks_back_through_requires_to_the_entry_that_started_it() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "project.acme")], &[]);

    let result = fixture.cli(&["why", &format!("skill:{CORE_SKILL}")]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        result.stdout,
        [
            format!("skill {CORE_SKILL}"),
            s(""),
            s("chain (3)"),
            format!(
                "  {}  pack   pack:{CATALOG_NAME}/project.acme",
                pad_end("project.acme", CORE_SKILL.len())
            ),
            format!(
                "  {}  skill  required-by:pack:project.acme",
                pad_end(PROJECT_SKILL, CORE_SKILL.len())
            ),
            format!("  {CORE_SKILL}  skill  required-by:skill:{PROJECT_SKILL}"),
        ]
        .join("\n")
    );
}

#[test]
fn why_names_the_entry_as_written_wildcard_included() {
    // The whole reason a reason carries the entry rather than the matched name: the wildcard is
    // what the reader can go and change, and the chain ends on it.
    let fixture = Fixture::new();

    fixture.write_profile(&engineering(), &[]);

    let result = fixture.cli(&["why", &format!("skill:{FRONTEND_SKILL}")]);

    assert!(
        result
            .stdout
            .contains(&format!("pack:{CATALOG_NAME}/function.engineering.*"))
    );
}

#[test]
fn why_finds_a_server_by_the_mcp_reference_requires_uses() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "project.acme")], &[]);

    let result = fixture.cli(&["why", "mcp:fixture"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert!(result.stdout.contains("mcp fixture"));
    assert!(result.stdout.contains(&format!(
        "{}  mcp    required-by:skill:{PROJECT_SKILL}",
        pad_end("fixture", "project.acme".len())
    )));
}

#[test]
fn why_refuses_a_bare_name() {
    // Refused rather than looked up, even though this command *could* look it up: one grammar
    // everywhere a name is taken from a person beats a rule that holds only while a name is unique.
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "core")], &[]);

    let result = fixture.cli(&["why", CORE_SKILL]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains(&format!("`why {CORE_SKILL}` does not say what to explain"))
    );
    assert!(
        result
            .stderr
            .contains(&format!("`pack:{CORE_SKILL}`, `skill:{CORE_SKILL}`"))
    );
}

#[test]
fn why_names_either_namespace_for_a_name_both_hold() {
    // Two namespaces answering to one name is a legitimate catalog, and neither reading is
    // preferred over the other: both are simply asked for.
    let fixture = Fixture::new();

    fixture.write_mcp(CORE_SKILL, &[]);
    fixture.in_core_pack(&[needs("mcp", CORE_SKILL)]);
    fixture.write_profile(&[entry("pack", "core")], &[]);

    assert!(
        fixture
            .cli(&["why", &format!("skill:{CORE_SKILL}")])
            .stdout
            .contains(&format!("skill {CORE_SKILL}"))
    );
    assert!(
        fixture
            .cli(&["why", &format!("mcp:{CORE_SKILL}")])
            .stdout
            .contains(&format!("mcp {CORE_SKILL}"))
    );
}

#[test]
fn why_reaches_a_skill_whose_name_reads_like_another_namespaces_prefix() {
    // `skills/mcp/sentry/SKILL.md` is the skill `mcp.sentry`, and under a prefix convention no
    // string could name it.
    let fixture = Fixture::new();

    fixture.write_skill("mcp.sentry", &[]);
    fixture.in_core_pack(&[needs("skill", "mcp.sentry")]);
    fixture.write_profile(&[entry("pack", "core")], &[]);

    assert!(
        fixture
            .cli(&["why", "skill:mcp.sentry"])
            .stdout
            .contains("skill mcp.sentry")
    );
}

#[test]
fn why_lets_a_skill_named_for_a_namespace_and_an_entity_of_that_name_coexist() {
    // Two different things, and both reachable: the kind decides, and the name never does.
    let fixture = Fixture::new();

    fixture.write_skill("mcp.sentry", &[]);
    fixture.write_mcp("sentry", &[]);
    fixture.in_core_pack(&[needs("skill", "mcp.sentry"), needs("mcp", "sentry")]);
    fixture.write_profile(&[entry("pack", "core")], &[]);

    assert!(
        fixture
            .cli(&["why", "skill:mcp.sentry"])
            .stdout
            .contains("skill mcp.sentry")
    );
    assert!(
        fixture
            .cli(&["why", "mcp:sentry"])
            .stdout
            .contains("mcp sentry")
    );
}

#[test]
fn why_prints_the_chain_to_a_hook_a_skill_required() {
    let fixture = Fixture::new();

    fixture.write_hook(
        "guard",
        &[
            "event: PreToolUse",
            "matcher: Bash",
            "type: command",
            "command: npx guard",
        ],
    );
    fixture.write_skill("risky", &[requires(&[needs("hook", "guard")])]);
    fixture.in_core_pack(&[needs("skill", "risky")]);
    fixture.write_profile(&[entry("pack", "core")], &[]);

    let result = fixture.cli(&["why", "hook:guard"]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert_eq!(
        result.stdout,
        [
            s("hook guard"),
            s(""),
            s("chain (3)"),
            format!("  core   pack   pack:{CATALOG_NAME}/core"),
            s("  risky  skill  required-by:pack:core"),
            s("  guard  hook   required-by:skill:risky"),
        ]
        .join("\n")
    );
}

#[test]
fn why_insists_on_the_hook_for_a_hook_reference_a_skill_also_answers_to() {
    let fixture = Fixture::new();

    fixture.write_hook(
        CORE_SKILL,
        &["event: Stop", "type: command", "command: npx notify"],
    );
    fixture.in_core_pack(&[needs("hook", CORE_SKILL)]);
    fixture.write_profile(&[entry("pack", "core")], &[]);

    assert!(
        fixture
            .cli(&["why", &format!("skill:{CORE_SKILL}")])
            .stdout
            .contains(&format!("skill {CORE_SKILL}"))
    );
    assert!(
        fixture
            .cli(&["why", &format!("hook:{CORE_SKILL}")])
            .stdout
            .contains(&format!("hook {CORE_SKILL}"))
    );
}

#[test]
fn why_names_the_entry_that_would_select_an_unselected_hook() {
    let fixture = Fixture::new();

    fixture.write_hook(
        "guard",
        &[
            "event: PreToolUse",
            "matcher: Bash",
            "type: command",
            "command: npx guard",
        ],
    );
    fixture.write_profile(&[entry("pack", "core")], &[]);

    let result = fixture.cli(&["why", "hook:guard"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("hook \"guard\" is not in the bundle")
    );
    // By exact name and qualified, which is the one entry that selects this copy and nothing else.
    assert!(result.stderr.contains(&format!(
        "select it with `- hook: \"{CATALOG_NAME}/guard\"`"
    )));
}

#[test]
fn why_reports_a_name_entry_as_the_whole_chain() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("skill", ENGINEERING_SKILL)], &[]);

    let result = fixture.cli(&["why", &format!("skill:{ENGINEERING_SKILL}")]);

    assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
    assert!(result.stdout.contains(&format!(
        "{ENGINEERING_SKILL}  skill  skill:{CATALOG_NAME}/{ENGINEERING_SKILL}"
    )));
}

#[test]
fn why_emits_the_chain_the_item_and_its_reason_as_json() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "project.acme")], &[]);

    let result = fixture.cli(&["why", "mcp:fixture", "--json"]);
    let parsed: Value = serde_json::from_str(&result.stdout).expect("JSON");

    assert_eq!(
        parsed,
        serde_json::json!({
            "chain": [
                {
                    "kind": "pack",
                    "name": "project.acme",
                    "reason": format!("pack:{CATALOG_NAME}/project.acme"),
                },
                { "kind": "skill", "name": PROJECT_SKILL, "reason": "required-by:pack:project.acme" },
                { "kind": "mcp", "name": "fixture", "reason": format!("required-by:skill:{PROJECT_SKILL}") },
            ],
            "kind": "mcp",
            "name": "fixture",
            "reason": format!("required-by:skill:{PROJECT_SKILL}"),
        })
    );
}

#[test]
fn why_exits_3_for_a_skill_a_catalog_provides_but_nothing_selects() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "core")], &[]);

    let result = fixture.cli(&["why", &format!("skill:{PROJECT_SKILL}")]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains(&format!("skill \"{PROJECT_SKILL}\" is not in the bundle"))
    );
    assert!(
        result
            .stderr
            .contains(&format!("catalog \"{CATALOG_NAME}\" provides it"))
    );
    assert!(result.stderr.contains(&format!(
        "select it with `- skill: \"{CATALOG_NAME}/{PROJECT_SKILL}\"`"
    )));
}

#[test]
fn why_names_an_entry_for_an_unselected_server_too() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "core")], &[]);

    let result = fixture.cli(&["why", "mcp:fixture"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("MCP server \"fixture\" is not in the bundle")
    );
    assert!(!result.stderr.contains("it declares tags"));
    assert!(result.stderr.contains(&format!(
        "select it with `- mcp: \"{CATALOG_NAME}/fixture\"`"
    )));
}

#[test]
fn why_exits_3_for_a_name_nothing_provides_naming_the_namespace() {
    // The namespace is named rather than hedged over all four: the subject said which it meant.
    let fixture = Fixture::new();
    let result = fixture.cli(&["why", "skill:absent-skill"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stderr.contains("unknown skill \"absent-skill\""));
    // The next step carries the name that was typed, so a misremembered one is one paste away.
    assert!(result.stderr.contains(
        "run `ambit search --capability skill \"*absent-skill*\"` to see what is available"
    ));
}

#[test]
fn why_does_not_fall_back_to_another_namespace() {
    // `company-context` is a skill this catalog does have. A reference is taken at its word, so
    // naming the wrong namespace is a miss rather than a lookup that wanders into the right one.
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "core")], &[]);

    let result = fixture.cli(&["why", &format!("mcp:{CORE_SKILL}")]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains(&format!("unknown MCP server \"{CORE_SKILL}\""))
    );
}

#[test]
fn why_refuses_a_subject_whose_kind_is_not_a_namespace() {
    // `server:` is no kind, so this is a bare name, and the refusal explains the grammar rather
    // than complaining about a namespace nobody claimed to be naming.
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "core")], &[]);

    let result = fixture.cli(&["why", "server:fixture"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(
        result
            .stderr
            .contains("`why server:fixture` does not say what to explain")
    );
    assert!(result.stderr.contains("`skill:server:fixture`"));
}

// Spec §4.6: an entry that matches nothing is exit 3, not a silent miss. The line assertions are
// exact rather than loose, since the whole point of the message is that it sends a reader to the
// offending line of their own config.

#[test]
fn unmatched_entry_exits_3_naming_the_entry_its_line_and_what_it_looked_in() {
    let fixture = Fixture::new();

    fixture.write_profile(
        &[entry("pack", "core"), entry("pack", "function.enginering")],
        &[],
    );

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stderr.contains(&format!(
        "`requires` entry \"pack:{CATALOG_NAME}/function.enginering\" matches nothing (ambit.yml line {})",
        FIRST_ENTRY_LINE + 1
    )));
    assert!(result.stderr.contains(&format!(
        "no pack in catalog \"{CATALOG_NAME}\" has a name matching \"function.enginering\""
    )));
    assert!(
        result
            .stderr
            .contains("correct the pattern, add the item to a catalog")
    );
}

#[test]
fn unmatched_wildcard_is_refused_exactly_as_a_misspelled_name() {
    // A stale glob and a typo'd exact name are the same mistake.
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("skill", "absent.*")], &[]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stderr.contains(&format!(
        "`requires` entry \"skill:{CATALOG_NAME}/absent.*\""
    )));
}

#[test]
fn unmatched_qualifier_says_it_names_no_catalog() {
    // A qualifier is an alias, not a pattern, so `*` in that half asks for a catalog literally
    // named `*`, and a message about what that catalog holds would answer the wrong question.
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("skill", "*/core")], &[]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("no catalog in `catalogs:` is named \"*\"")
    );
    assert!(result.stderr.contains("`*` is matched literally there"));
    assert!(
        result
            .stderr
            .contains(&format!("configured catalogs: {CATALOG_NAME}"))
    );
}

#[test]
fn unmatched_alias_is_named_without_the_wildcard_aside() {
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("pack", "compny/core")], &[]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(
        result
            .stderr
            .contains("no catalog in `catalogs:` is named \"compny\"")
    );
    assert!(!result.stderr.contains("matched literally"));
    assert!(
        result
            .stderr
            .contains("correct the qualifier, or add the catalog to `catalogs:`")
    );
}

#[test]
fn unmatched_namespace_is_refused_however_live_the_name_is_elsewhere() {
    // `core` is a pack, and there is no *skill* of that name.
    let fixture = Fixture::new();

    fixture.write_profile(&[entry("skill", "core")], &[]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Resolution);
    assert!(result.stderr.contains(&format!(
        "no skill in catalog \"{CATALOG_NAME}\" has a name matching \"core\""
    )));
}

#[test]
fn unmatched_reports_the_same_offender_however_the_config_orders_the_list() {
    let fixture = Fixture::new();

    fixture.write_profile(
        &[
            entry("pack", "zeta.unknown"),
            entry("pack", "alpha.unknown"),
        ],
        &[],
    );
    let first = fixture.cli(&["resolve"]);

    fixture.write_profile(
        &[
            entry("pack", "alpha.unknown"),
            entry("pack", "zeta.unknown"),
        ],
        &[],
    );
    let second = fixture.cli(&["resolve"]);

    let expected = format!("\"pack:{CATALOG_NAME}/alpha.unknown\" matches nothing");

    assert!(first.stderr.contains(&expected));
    assert!(second.stderr.contains(&expected));
}

#[test]
fn unmatched_selects_nothing_before_failing_so_install_cannot_half_run() {
    let fixture = Fixture::new();
    let error = fixture
        .try_bundle(&[entry("pack", "core"), entry("pack", "not.a.tag")])
        .expect_err("refused");

    assert_eq!(error.code, ExitCode::Resolution);
}

// `ambit resolve`.

#[test]
fn resolve_lists_the_bundle_as_text() {
    let fixture = Fixture::new();

    fixture.write_profile(&core_and_engineering(), &[]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Success);
    let pack = FRONTEND_PACK.len();
    let skill = CORE_SKILL.len();

    assert_eq!(
        result.stdout,
        [
            // The catalog column is padded out to the widest name, so it lines up down the section.
            s("packs (3)"),
            format!("  {}  {CATALOG_NAME}", pad_end("core", pack)),
            format!(
                "  {}  {CATALOG_NAME}",
                pad_end("function.engineering", pack)
            ),
            format!("  {FRONTEND_PACK}  {CATALOG_NAME}"),
            s(""),
            s("skills (3)"),
            format!("  {}  {CATALOG_NAME}", pad_end(ENGINEERING_SKILL, skill)),
            format!("  {CORE_SKILL}  {CATALOG_NAME}"),
            format!("  {}  {CATALOG_NAME}", pad_end(FRONTEND_SKILL, skill)),
            s(""),
            s("mcps (1)"),
            format!("  linter  {CATALOG_NAME}"),
            s(""),
            s("hooks (2)"),
            format!("  {ENGINEERING_HOOK}  {CATALOG_NAME}  PreToolUse"),
            format!("  {CORE_HOOK}  {CATALOG_NAME}  SessionStart"),
            s(""),
            s("expects (2)"),
            s("  env  ACME_FIGMA_TOKEN"),
            s("  env  LINTER_API_KEY"),
        ]
        .join("\n")
    );
}

#[test]
fn resolve_says_so_for_an_empty_bundle_rather_than_printing_nothing() {
    let fixture = Fixture::new();

    fixture.write_profile(&[], &[]);

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Success);
    assert_eq!(
        result.stdout,
        [
            "packs (0)",
            "  (none)",
            "",
            "skills (0)",
            "  (none)",
            "",
            "mcps (0)",
            "  (none)",
            "",
            "hooks (0)",
            "  (none)",
            "",
            "expects (0)",
            "  (none)",
        ]
        .join("\n")
    );
}

#[test]
fn resolve_emits_byte_identical_json_on_a_second_run() {
    let fixture = Fixture::new();

    fixture.write_profile(&core_and_engineering(), &[]);

    let first = fixture.cli(&["resolve", "--json"]);
    let second = fixture.cli(&["resolve", "--json"]);

    assert_eq!(second.stdout, first.stdout);
}

#[test]
fn resolve_carries_no_machine_specific_paths_into_json_output() {
    let fixture = Fixture::new();
    let result = fixture.cli(&["resolve", "--json"]);

    assert!(!result.stdout.contains(&*fixture.root.to_string_lossy()));
}

#[test]
fn resolve_emits_byte_identical_json_on_a_second_run_under_explain_too() {
    let fixture = Fixture::new();
    let mut requires = core_and_engineering();

    requires.push(entry("pack", "project.acme"));
    fixture.write_profile(&requires, &[]);

    let first = fixture.cli(&["resolve", "--explain", "--json"]);
    let second = fixture.cli(&["resolve", "--explain", "--json"]);

    assert_eq!(second.stdout, first.stdout);
}

#[test]
fn resolve_exits_2_when_the_project_has_no_config() {
    let fixture = Fixture::new();

    fs::remove_file(fixture.project_dir.join("ambit.yml")).expect("remove ambit.yml");

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("no ambit config"));
}

#[test]
fn resolve_exits_2_on_a_malformed_catalog() {
    let fixture = Fixture::new();

    write(
        &fixture.catalog_dir.join("mcps/broken.yml"),
        "name: broken\n",
    );

    let result = fixture.cli(&["resolve"]);

    assert_eq!(result.code, ExitCode::Config);
    assert!(result.stderr.contains("missing required key \"transport\""));
}
