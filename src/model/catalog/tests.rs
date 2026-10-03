//! Catalog parsing from a `path:` source, and the `ambit search` view built on it.
//!
//! Every case runs against the fixture catalog, mutated in place for the malformed ones, so the
//! subject is the same tree the rest of the suite resolves against.
//!
//! The CLI mechanism cases (the command surface, the nested-command seam, usage errors, the flag
//! rules) live in `src/cli/tests.rs`.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::errors::ExitCode;
use crate::model::config::load_project_config;
use crate::model::expectation::ExpectationKind;
use crate::model::mcp_entity::{HttpTransport, StdioTransport};
use crate::model::reference::Reference;
use crate::model::requirement::ItemKind;
use crate::test_support::fixture_catalog::build_fixture_catalog;
use crate::test_support::{CliResult, run_cli, tempdir, test_env};
use crate::util::fs::{copy_tree, mkdir_p, rm_rf, write_text};

const CATALOG_NAME: &str = "company";
const CODE_REVIEW: &str = "skills/code-review/SKILL.md";

/// The fixture's core skill, and the pack a second catalog collides with.
const CORE_SKILL: &str = "company-context";
const CORE_TAG: &str = "core";

/// A skill only the second catalog provides, so a merge has something to keep from both.
const OWN_SKILL: &str = "jane-notes";

/// A fixture catalog beside a project that lists it as `path:../catalog`.
struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    catalog_dir: PathBuf,
    project_dir: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempdir();
        let root = dir.path().to_path_buf();
        let catalog_dir = root.join("catalog");
        let project_dir = root.join("project");

        build_fixture_catalog(&catalog_dir).unwrap();
        mkdir_p(&project_dir).unwrap();

        let fixture = Self {
            _dir: dir,
            root,
            catalog_dir,
            project_dir,
        };

        fixture.write_config(&format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\n"
        ));
        fixture
    }

    /// What source resolution reads from outside its arguments; every source here is a local
    /// path.
    fn context(&self) -> SourceContext {
        SourceContext {
            project_dir: self.project_dir.clone(),
            env: test_env(&self.root),
            offline: false,
        }
    }

    /// Rewrites `ambit.yml` for the project under test.
    fn write_config(&self, body: &str) {
        write_text(&self.project_dir.join("ambit.yml"), body).unwrap();
    }

    /// Replaces one file inside the fixture catalog.
    fn write_catalog_file(&self, relative: &str, body: &str) {
        write_file(&self.catalog_dir.join(relative), body);
    }

    fn parse(&self) -> Result<Catalog> {
        parse_catalog_directory(
            CATALOG_NAME,
            "path:../catalog",
            &self.catalog_dir,
            None,
            &mut CatalogParseOptions::default(),
        )
    }

    /// Parses the fixture catalog, asserting it was rejected as a config error (exit 2).
    fn rejection(&self) -> AmbitError {
        let error = self
            .parse()
            .expect_err("expected the catalog to be rejected");

        assert_eq!(error.code, ExitCode::Config, "{}", error.format());
        error
    }

    /// Builds a catalog beside the fixture that deliberately collides with it: the same core skill
    /// and the same `linter` server, plus a skill of its own so the merge has something only one
    /// catalog provides. `name` is the catalog's directory, which is also the name config gives it.
    fn write_colliding_catalog(&self, name: &str) {
        let files = [
            (
                format!("skills/{}/SKILL.md", CORE_SKILL.replace('.', "/")),
                [
                    "---".to_owned(),
                    format!("name: {CORE_SKILL}"),
                    format!("description: {name}'s copy of the core skill."),
                    "---".to_owned(),
                    String::new(),
                    format!("# {name}'s copy"),
                    String::new(),
                ]
                .join("\n"),
            ),
            (
                format!("skills/{}/SKILL.md", OWN_SKILL.replace('.', "/")),
                [
                    "---".to_owned(),
                    format!("name: {OWN_SKILL}"),
                    "description: Jane's notes, which no other catalog provides.".to_owned(),
                    "---".to_owned(),
                    String::new(),
                    "# notes".to_owned(),
                    String::new(),
                ]
                .join("\n"),
            ),
            (
                "mcps/linter.yml".to_owned(),
                [
                    "name: linter".to_owned(),
                    "transport:".to_owned(),
                    "  stdio:".to_owned(),
                    format!("    command: {name}-mcp"),
                    String::new(),
                ]
                .join("\n"),
            ),
            // Every catalog offers the same two groupings by name, which is what makes a project
            // selecting from both a collision rather than two different asks.
            (
                format!("packs/{CORE_TAG}.yml"),
                [
                    format!("name: {CORE_TAG}"),
                    format!("description: {name}'s core pack."),
                    "requires:".to_owned(),
                    format!("  - skill: {CORE_SKILL}"),
                    format!("  - skill: {OWN_SKILL}"),
                    String::new(),
                ]
                .join("\n"),
            ),
            (
                "packs/person.jane.yml".to_owned(),
                [
                    "name: person.jane".to_owned(),
                    format!("description: {name}'s pack for Jane."),
                    "requires:".to_owned(),
                    format!("  - skill: {OWN_SKILL}"),
                    String::new(),
                ]
                .join("\n"),
            ),
        ];

        for (relative, body) in files {
            write_file(&self.root.join(name).join(relative), &body);
        }
    }

    /// Points the project at the fixture catalog first and the extra catalogs after it.
    ///
    /// The order is only how the file reads: nothing resolves by it, since every catalog's copy of
    /// a name survives the merge. Each `requires` line carries its own qualifier.
    fn write_catalog_order(&self, extra: &[&str], requires: &[String]) {
        let mut lines = vec![
            "version: 1".to_owned(),
            "catalogs:".to_owned(),
            format!("  - name: {CATALOG_NAME}"),
            "    source: path:../catalog".to_owned(),
        ];

        for name in extra {
            lines.push(format!("  - name: {name}"));
            lines.push(format!("    source: path:../{name}"));
        }

        if requires.is_empty() {
            lines.push("requires: []".to_owned());
        } else {
            lines.push("requires:".to_owned());
            lines.extend(requires.iter().cloned());
        }

        lines.push(String::new());
        self.write_config(&lines.join("\n"));
    }

    /// The merged view of whatever the project's config currently lists.
    fn merged(&self) -> MergedCatalog {
        let config = load_project_config(&self.project_dir).unwrap();

        merge_catalogs(
            &load_catalogs(&config, &self.context(), &mut CatalogLoadOptions::default()).unwrap(),
        )
    }

    /// Runs the CLI against the project under test, from the fixture root.
    fn cli(&self, args: &[&str]) -> CliResult {
        let project = self.project_dir.to_string_lossy().into_owned();
        let mut argv: Vec<&str> = args.to_vec();

        argv.extend(["--project", &project]);
        run_cli(&argv, &self.root, &test_env(&self.root))
    }
}

fn write_file(target: &Path, body: &str) {
    mkdir_p(target.parent().unwrap()).unwrap();
    write_text(target, body).unwrap();
}

/// One `requires` entry, taking a whole pack from `catalog`.
fn requires_entry(pack: &str, catalog: &str) -> String {
    format!("  - {{ pack: \"{catalog}/{pack}\" }}")
}

fn names<T>(items: &[T], name: impl Fn(&T) -> &str) -> Vec<String> {
    items.iter().map(|item| name(item).to_owned()).collect()
}

fn json_of(stdout: &str) -> Value {
    serde_json::from_str(stdout).unwrap_or_else(|error| panic!("{error}: {stdout}"))
}

mod catalog_parsing {
    use super::*;

    #[test]
    fn reads_every_skill_and_every_mcp_entity_and_nothing_at_the_root() {
        let fixture = Fixture::new();
        let catalog = fixture.parse().unwrap();

        assert_eq!(catalog.name, CATALOG_NAME);
        assert_eq!(catalog.root, fixture.catalog_dir);
        assert_eq!(
            names(&catalog.skills, |skill| &skill.name),
            [
                "acme-brief",
                "code-review",
                "company-context",
                "design-tokens"
            ]
        );
        assert_eq!(names(&catalog.mcps, |mcp| &mcp.name), ["fixture", "linter"]);
    }

    #[test]
    fn derives_each_skills_name_and_path_from_its_directory() {
        let catalog = Fixture::new().parse().unwrap();
        let frontend = catalog
            .skills
            .iter()
            .find(|skill| skill.name == "design-tokens")
            .unwrap();

        assert_eq!(frontend.path, "skills/design-tokens");
        assert_eq!(frontend.requires, Vec::<PatternEntry>::new());
        assert_eq!(
            frontend.expects,
            [Reference {
                kind: ExpectationKind::Env,
                name: "ACME_FIGMA_TOKEN".to_owned(),
            }]
        );
        assert!(
            frontend
                .description
                .as_deref()
                .is_some_and(|text| !text.is_empty())
        );
    }

    #[test]
    fn joins_a_nested_skills_path_segments_with_a_dot() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            "skills/personal/notes/SKILL.md",
            "---\nname: personal.notes\ndescription: x\n---\n",
        );

        let catalog = fixture.parse().unwrap();
        let notes = catalog
            .skills
            .iter()
            .find(|skill| skill.name == "personal.notes")
            .unwrap();

        assert_eq!(notes.path, "skills/personal/notes");
    }

    #[test]
    fn carries_requires_through_as_pattern_entries_unqualified() {
        let catalog = Fixture::new().parse().unwrap();
        let entry = |kind, pattern: &str| PatternEntry {
            kind,
            pattern: pattern.to_owned(),
            catalog: None,
        };

        // In the order the fixture wrote them: a `requires` list is the author's, not a sorted
        // one. No `catalog` on any entry: a catalog author cannot write a consumer's alias.
        assert_eq!(
            catalog
                .skills
                .iter()
                .find(|skill| skill.name == "acme-brief")
                .unwrap()
                .requires,
            [
                entry(ItemKind::Skill, "company-context"),
                entry(ItemKind::Mcp, "fixture"),
                entry(ItemKind::Hook, "acme-standup"),
            ]
        );
    }

    #[test]
    fn parses_both_transport_kinds() {
        let catalog = Fixture::new().parse().unwrap();
        let transport = |name: &str| {
            catalog
                .mcps
                .iter()
                .find(|mcp| mcp.name == name)
                .unwrap()
                .transport
                .clone()
        };

        assert_eq!(
            transport("fixture"),
            McpTransport::Stdio(StdioTransport {
                command: "npx".to_owned(),
                args: vec!["-y".to_owned(), "@acme/fixture-mcp".to_owned()],
                env: IndexMap::new(),
            })
        );
        assert_eq!(
            transport("linter"),
            McpTransport::Http(HttpTransport {
                url: "https://mcp.invalid/fixture".to_owned(),
                bearer_token_env_var: Some("LINTER_API_KEY".to_owned()),
                headers: IndexMap::new(),
            })
        );
    }

    #[test]
    fn keeps_top_level_frontmatter_keys_it_does_not_know() {
        // The top level is the harness's; ambit adds exactly one key to it.
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            CODE_REVIEW,
            "---
name: code-review
description: x
allowed-tools: [Read, Grep]
ambit:
  requires: [{ skill: company-context }]
---
",
        );

        assert!(
            names(&fixture.parse().unwrap().skills, |skill| &skill.name)
                .contains(&"code-review".to_owned())
        );
    }

    #[test]
    fn parses_identically_whatever_order_the_filesystem_reports() {
        let fixture = Fixture::new();
        let first = fixture.parse().unwrap();
        let other = fixture.root.join("reordered");

        copy_tree(&fixture.catalog_dir, &other).unwrap();

        let second = parse_catalog_directory(
            CATALOG_NAME,
            "path:../catalog",
            &other,
            None,
            &mut CatalogParseOptions::default(),
        )
        .unwrap();

        assert_eq!(
            Catalog {
                root: fixture.catalog_dir.clone(),
                ..second
            },
            first
        );
    }
}

mod catalog_parsing_failures {
    use super::*;

    #[test]
    fn rejects_a_skill_whose_frontmatter_name_disagrees_with_its_path() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(CODE_REVIEW, "---\nname: wrong-name\ndescription: x\n---\n");

        let error = fixture.rejection();

        assert!(
            error
                .message
                .contains("skill name \"wrong-name\" does not match its path")
        );
        // The line is the one the reader will find `name` on in the whole document, not in the
        // block.
        assert!(error.message.contains(&format!("{CODE_REVIEW} line 2")));
        assert!(
            error
                .detail
                .join("\n")
                .contains("derives the name \"code-review\"")
        );
    }

    #[test]
    fn collects_a_name_disagreement_and_takes_the_paths_answer() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(CODE_REVIEW, "---\nname: wrong-name\ndescription: x\n---\n");

        let mut collected = Vec::new();
        let catalog = parse_catalog_directory(
            CATALOG_NAME,
            "path:../catalog",
            &fixture.catalog_dir,
            None,
            &mut CatalogParseOptions {
                collect: Some(&mut collected),
            },
        )
        .unwrap();

        assert!(names(&catalog.skills, |skill| &skill.name).contains(&"code-review".to_owned()));
        assert_eq!(collected.len(), 1);
        // Named by catalog, not by root: a collected problem is printed in a report compared byte
        // for byte across machines.
        assert_eq!(
            collected[0].detail[0],
            format!("in catalog \"{CATALOG_NAME}\"")
        );
    }

    #[test]
    fn rejects_a_key_under_ambit_the_format_does_not_define() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            CODE_REVIEW,
            "---\nname: code-review\ndescription: x\nambit:\n  tag: [function.engineering]\n---\n",
        );

        let error = fixture.rejection();

        assert_eq!(
            error.message,
            format!("unknown key \"ambit.tag\" ({CODE_REVIEW} line 5)")
        );
        assert!(
            error
                .detail
                .contains(&"accepted keys: expects, requires".to_owned())
        );
    }

    #[test]
    fn rejects_an_ambit_that_is_not_a_mapping() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            CODE_REVIEW,
            "---\nname: code-review\ndescription: x\nambit: [function.engineering]\n---\n",
        );

        assert_eq!(
            fixture.rejection().message,
            format!("\"ambit\" must be a mapping ({CODE_REVIEW} line 4)")
        );
    }

    #[test]
    fn positions_a_frontmatter_error_at_its_line_in_the_document() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            CODE_REVIEW,
            "---\nname: code-review\ndescription: x\ndescription: y\n---\n",
        );

        assert_eq!(
            fixture.rejection().message,
            format!("duplicate key \"description\" ({CODE_REVIEW} line 4)")
        );
    }

    #[test]
    fn rejects_a_skill_with_no_frontmatter() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(CODE_REVIEW, "# just a document\n");

        assert!(
            fixture
                .rejection()
                .message
                .contains(&format!("{CODE_REVIEW} has no frontmatter block"))
        );
    }

    #[test]
    fn rejects_an_empty_frontmatter_block() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(CODE_REVIEW, "---\n---\nbody\n");

        assert!(
            fixture
                .rejection()
                .message
                .contains(&format!("{CODE_REVIEW} has an empty frontmatter block"))
        );
    }

    #[test]
    fn rejects_frontmatter_that_is_not_yaml() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(CODE_REVIEW, "---json\n{\"name\": \"x\"}\n---\n");

        assert!(
            fixture
                .rejection()
                .message
                .contains("declares its frontmatter as \"json\"")
        );
    }

    #[test]
    fn rejects_an_mcp_entity_whose_name_disagrees_with_its_filename() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            "mcps/other.yml",
            "name: notother\ntransport:\n  stdio:\n    command: npx\n",
        );

        let error = fixture.rejection();

        assert!(
            error
                .message
                .contains("MCP name \"notother\" does not match its filename")
        );
        assert!(
            error
                .detail
                .join("\n")
                .contains("declares the name \"other\"")
        );
    }

    /// The transport rules, read through the file that is the only place a server can be written.
    #[test]
    fn rejects_an_mcp_entity_whose_transport_is_malformed() {
        let cases = [
            (
                "names no kind",
                "transport: {}\n",
                "`transport` names no transport kind",
            ),
            (
                "names two kinds",
                "transport:\n  stdio:\n    command: npx\n  http:\n    url: https://x.invalid\n",
                "`transport` names 2 transport kinds: http, stdio",
            ),
            (
                "names a kind ambit does not have",
                "transport:\n  sse:\n    url: https://x.invalid\n",
                "unknown transport kind \"sse\"",
            ),
            (
                "gives a stdio transport no command",
                "transport:\n  stdio: {}\n",
                "missing required key \"transport.stdio.command\"",
            ),
            (
                "spells a stdio transport's env map some other way",
                "transport:\n  stdio:\n    command: npx\n    environment:\n      TOKEN: x\n",
                "unknown key \"transport.stdio.environment\"",
            ),
            (
                "gives an env entry something other than a string",
                "transport:\n  stdio:\n    command: npx\n    env:\n      TOKEN: [x]\n",
                "\"transport.stdio.env.TOKEN\" must be a string",
            ),
        ];

        for (label, body, expected) in cases {
            let fixture = Fixture::new();

            fixture.write_catalog_file("mcps/broken.yml", &format!("name: broken\n{body}"));

            let formatted = fixture.rejection().format();

            assert!(formatted.contains(expected), "{label}: {formatted}");
        }
    }

    #[test]
    fn lists_the_transport_kinds_it_does_have_when_one_is_missing() {
        let fixture = Fixture::new();

        fixture.write_catalog_file("mcps/broken.yml", "name: broken\ntransport: {}\n");

        assert!(
            fixture
                .rejection()
                .format()
                .contains("supported kinds: http, stdio")
        );
    }

    #[test]
    fn rejects_one_mcp_name_defined_by_two_files() {
        let fixture = Fixture::new();

        copy_tree(
            &fixture.catalog_dir.join("mcps/fixture.yml"),
            &fixture.catalog_dir.join("mcps/fixture.yaml"),
        )
        .unwrap();

        assert_eq!(
            fixture.rejection().message,
            "mcps/fixture.yaml and mcps/fixture.yml both define \"fixture\""
        );
    }

    #[test]
    fn rejects_one_pack_name_defined_by_a_nested_file_and_a_dotted_one() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            "packs/function.engineering.yml",
            "name: function.engineering\n",
        );

        let error = fixture.rejection();

        assert_eq!(
            error.message,
            "packs/function/engineering.yml and packs/function.engineering.yml both define \"function.engineering\""
        );
        assert!(
            error
                .detail
                .contains(&"delete one, keeping packs/function.engineering.yml".to_owned())
        );
    }

    #[test]
    fn refuses_a_catalog_that_still_holds_a_scopes_yml_naming_the_rewrite() {
        // The registry is the one thing at a catalog root ambit still has an opinion about, and
        // the opinion is that it must not be there.
        let fixture = Fixture::new();

        fixture.write_catalog_file("scopes.yml", "scopes:\n  core:\n    description: A\n");

        let error = fixture.rejection();

        assert_eq!(error.message, "the scope registry is gone (scopes.yml)");
        assert!(
            error
                .detail
                .join("\n")
                .contains("a group of items is a pack now")
        );
        assert!(error.detail.join("\n").contains("selected with `pack:`"));
    }

    #[test]
    fn ignores_an_ambit_yml_at_the_catalog_root() {
        let fixture = Fixture::new();

        fixture.write_catalog_file("ambit.yml", "version: 1\nrequires: []\n");

        assert!(
            names(&fixture.parse().unwrap().skills, |skill| &skill.name)
                .contains(&"company-context".to_owned())
        );
    }

    #[test]
    fn names_the_catalog_an_error_came_from() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(CODE_REVIEW, "# no frontmatter\n");

        assert_eq!(
            fixture.rejection().detail[0],
            format!(
                "in catalog \"{CATALOG_NAME}\" ({})",
                fixture.catalog_dir.display()
            )
        );
    }
}

mod catalog_sources {
    use super::*;

    #[test]
    fn resolves_a_path_source_relative_to_the_project() {
        let fixture = Fixture::new();
        let config = load_project_config(&fixture.project_dir).unwrap();
        let catalogs = load_catalogs(
            &config,
            &fixture.context(),
            &mut CatalogLoadOptions::default(),
        )
        .unwrap();

        assert_eq!(
            catalogs
                .iter()
                .map(|catalog| catalog.root.clone())
                .collect::<Vec<_>>(),
            std::slice::from_ref(&fixture.catalog_dir)
        );
        // A directory has no revision, so nothing pretends to pin one.
        assert_eq!(catalogs[0].commit, None);
    }

    #[test]
    fn rejects_a_source_in_no_recognized_format() {
        let fixture = Fixture::new();

        fixture.write_config(&format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: ../catalog\n"
        ));

        let result = fixture.cli(&["search", "*"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(result.stderr.contains(&format!(
            "catalog \"{CATALOG_NAME}\" has an unrecognized source"
        )));
        assert!(
            result
                .stderr
                .contains("use owner/repo, a git URL, `git:<url>`, or `path:./dir`")
        );
    }

    #[test]
    fn rejects_a_path_source_that_is_not_a_directory() {
        let fixture = Fixture::new();

        fixture.write_config(&format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../missing\n"
        ));

        let result = fixture.cli(&["search", "*"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(
            result
                .stderr
                .contains(&format!("catalog \"{CATALOG_NAME}\" is not a directory"))
        );
    }
}

mod ambit_search {
    use super::*;

    #[test]
    fn emits_the_full_fixture_catalog_as_json() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "*", "--json"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(
            json_of(&result.stdout),
            json!({
                "catalogs": [CATALOG_NAME],
                // The fixture's three: one the `core` pack names, one shipping a script, and one
                // in no pack.
                "hooks": {
                    "company/acme-standup": {
                        "catalog": CATALOG_NAME,
                        "type": "command",
                        "command": "echo \"acme session ended\"",
                        "description": "Records what the session touched, for the Acme standup.",
                        "expects": [],
                        "event": "SessionEnd",
                        "path": "hooks/acme-standup",
                    },
                    "company/guard-secrets": {
                        "catalog": CATALOG_NAME,
                        "type": "script",
                        "command": "guard.sh",
                        "description": "Inspects a Bash command before Acme's tooling runs it.",
                        "expects": [],
                        "event": "PreToolUse",
                        "matcher": "Bash",
                        "path": "hooks/guard-secrets",
                        "timeout": 10,
                    },
                    "company/session-notes": {
                        "catalog": CATALOG_NAME,
                        "type": "command",
                        "command": "echo \"acme conventions apply\"",
                        "description": "Reminds a session that Acme's conventions apply.",
                        "expects": [],
                        "event": "SessionStart",
                        "path": "hooks/session-notes",
                    },
                },
                "skills": {
                    "company/company-context": {
                        "catalog": CATALOG_NAME,
                        "description": "Canonical context about Acme — what it sells, to whom, and how it works.",
                        "expects": [],
                        "path": "skills/company-context",
                        "requires": [],
                    },
                    "company/design-tokens": {
                        "catalog": CATALOG_NAME,
                        "description": "Acme's design tokens — color, spacing, and the type scale.",
                        "expects": [{ "kind": "env", "name": "ACME_FIGMA_TOKEN" }],
                        "path": "skills/design-tokens",
                        "requires": [],
                    },
                    "company/code-review": {
                        "catalog": CATALOG_NAME,
                        "description": "How Acme reviews code — what reviewers look for, and in what order.",
                        "expects": [],
                        "path": "skills/code-review",
                        "requires": [],
                    },
                    "company/acme-brief": {
                        "catalog": CATALOG_NAME,
                        "description": "The Acme engagement brief — remit, contacts, and conventions.",
                        "expects": [],
                        "path": "skills/acme-brief",
                        "requires": [
                            { "kind": "skill", "pattern": "company-context" },
                            { "kind": "mcp", "pattern": "fixture" },
                            { "kind": "hook", "pattern": "acme-standup" },
                        ],
                    },
                },
                // What a pack is for, and what it gathers.
                "packs": {
                    "company/core": {
                        "catalog": CATALOG_NAME,
                        "description": "What every Acme session needs, whoever is in it.",
                        "requires": [
                            { "kind": "skill", "pattern": "company-context" },
                            { "kind": "hook", "pattern": "session-notes" },
                        ],
                    },
                    "company/function.engineering": {
                        "catalog": CATALOG_NAME,
                        "description": "Everything an Acme engineer needs — reviews, tooling, and the guards around them.",
                        "requires": [
                            { "kind": "pack", "pattern": "core" },
                            { "kind": "skill", "pattern": "code-review" },
                            { "kind": "mcp", "pattern": "linter" },
                            { "kind": "hook", "pattern": "guard-secrets" },
                        ],
                    },
                    "company/function.engineering.frontend": {
                        "catalog": CATALOG_NAME,
                        "description": "What an Acme engineer working on interfaces needs on top of the engineering pack.",
                        "requires": [
                            { "kind": "pack", "pattern": "function.engineering" },
                            { "kind": "skill", "pattern": "design-tokens" },
                        ],
                    },
                    "company/project.acme": {
                        "catalog": CATALOG_NAME,
                        "description": "The Acme engagement — its brief, and whatever the brief drags in.",
                        "requires": [{ "kind": "skill", "pattern": "acme-brief" }],
                    },
                },
                "mcps": {
                    "company/fixture": {
                        "catalog": CATALOG_NAME,
                        "expects": [{ "kind": "env", "name": "FIXTURE_API_KEY" }],
                        "transport": {
                            "kind": "stdio",
                            "command": "npx",
                            "args": ["-y", "@acme/fixture-mcp"],
                            "env": {},
                        },
                    },
                    "company/linter": {
                        "catalog": CATALOG_NAME,
                        "expects": [{ "kind": "env", "name": "LINTER_API_KEY" }],
                        "transport": {
                            "kind": "http",
                            "url": "https://mcp.invalid/fixture",
                            "bearer_token_env_var": "LINTER_API_KEY",
                            "headers": {},
                        },
                    },
                },
            })
        );
    }

    #[test]
    fn emits_byte_identical_json_on_a_second_run_with_keys_sorted() {
        let fixture = Fixture::new();
        let first = fixture.cli(&["search", "*", "--json"]);
        let second = fixture.cli(&["search", "*", "--json"]);

        assert_eq!(second.stdout, first.stdout);

        let emitted = json_of(&first.stdout);
        let keys: Vec<&String> = emitted.as_object().unwrap().keys().collect();
        let mut sorted = keys.clone();

        sorted.sort();
        assert_eq!(keys, sorted);
    }

    #[test]
    fn carries_no_machine_specific_paths_into_json_output() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "*", "--json"]);

        assert!(!result.stdout.contains(&*fixture.root.to_string_lossy()));
    }

    #[test]
    fn lists_packs_with_their_descriptions_and_skills_and_mcps_as_text() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "*"]);

        assert_eq!(result.code, ExitCode::Success);
        assert!(
            result
                .stdout
                .contains(&format!("{CATALOG_NAME}  path:../catalog"))
        );
        // The packs lead, and each carries what it is for.
        assert!(result.stdout.contains(&format!(
            "core                           {CATALOG_NAME}  What every Acme session needs"
        )));
        assert!(
            result
                .stdout
                .contains(&format!("company-context  {CATALOG_NAME}"))
        );
        assert!(result.stdout.contains("acme-brief"));
        assert!(result.stdout.contains("stdio: npx -y @acme/fixture-mcp"));
        assert!(result.stdout.contains("http: https://mcp.invalid/fixture"));
    }

    #[test]
    fn succeeds_with_nothing_to_dump_when_no_catalogs_are_configured() {
        let fixture = Fixture::new();

        fixture.write_config("version: 1\nrequires: []\n");

        let result = fixture.cli(&["search", "*"]);

        assert_eq!(result.code, ExitCode::Success);
        assert!(result.stdout.contains("no catalogs configured"));
    }

    #[test]
    fn exits_2_on_a_skill_name_that_disagrees_with_its_path() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(CODE_REVIEW, "---\nname: wrong-name\n---\n");

        let result = fixture.cli(&["search", "*", "--json"]);

        assert_eq!(result.code, ExitCode::Config);
        assert_eq!(result.stdout, "");
        assert!(result.stderr.contains("does not match its path"));
    }

    #[test]
    fn exits_2_when_the_project_has_no_config() {
        let fixture = Fixture::new();

        rm_rf(&fixture.project_dir.join("ambit.yml")).unwrap();

        let result = fixture.cli(&["search", "*"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(result.stderr.contains("no ambit config"));
    }
}

/// The three filters `ambit search` narrows with, and how they combine.
///
/// Repeating one flag widens and different flags narrow: a result has to satisfy every flag that
/// was given, not merely one of them. The pattern is the same glob a `requires` entry is written
/// with, so a person can paste what they typed into `requires:` and reach the same items. Matching
/// nothing is exit 0 and a report, where the same pattern in a `requires` entry is exit 3.
mod ambit_search_narrowed {
    use super::*;

    /// The section body `ambit search` prints under `title`, without its heading or its indent.
    fn rows_under(stdout: &str, title: &str) -> Vec<String> {
        let lines: Vec<&str> = stdout.split('\n').collect();
        let Some(start) = lines
            .iter()
            .position(|line| line.starts_with(&format!("{title} (")))
        else {
            return Vec::new();
        };

        lines[start + 1..]
            .iter()
            .take_while(|line| line.starts_with("  "))
            .map(|line| line.trim().to_owned())
            .collect()
    }

    /// The first column of each row.
    fn first_column(rows: &[String]) -> Vec<String> {
        rows.iter()
            .map(|row| row.split("  ").next().unwrap().to_owned())
            .collect()
    }

    /// Whether a section was printed at all, which is what `--capability` decides.
    fn has_section(stdout: &str, title: &str) -> bool {
        stdout
            .split('\n')
            .any(|line| line.starts_with(&format!("{title} (")))
    }

    #[test]
    fn matches_names_with_the_glob_a_requires_entry_uses_prefix_included() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "function.*", "--capability", "pack"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(
            first_column(&rows_under(&result.stdout, "packs")),
            ["function.engineering", "function.engineering.frontend"]
        );
    }

    #[test]
    fn excludes_the_prefix_itself_exactly_as_the_same_pattern_does_in_requires() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "function.engineering.*", "--capability", "pack"]);

        assert_eq!(
            first_column(&rows_under(&result.stdout, "packs")),
            ["function.engineering.frontend"]
        );
    }

    #[test]
    fn treats_a_pattern_with_no_wildcard_as_an_exact_name() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "code-review", "--capability", "skill"]);

        assert_eq!(
            first_column(&rows_under(&result.stdout, "skills")),
            ["code-review"]
        );
    }

    #[test]
    fn prints_only_the_sections_capability_asked_for() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "*", "--capability", "skill"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert!(has_section(&result.stdout, "skills"));
        // Omitted rather than printed empty: a section shown as `(none)` reads as *this catalog
        // has no packs*, which is a different answer from *you did not ask about packs*.
        for title in ["packs", "mcps", "hooks"] {
            assert!(!has_section(&result.stdout, title), "{title}");
        }
    }

    #[test]
    fn widens_when_capability_is_repeated() {
        let fixture = Fixture::new();
        let result = fixture.cli(&[
            "search",
            "*",
            "--capability",
            "skill",
            "--capability",
            "mcp",
        ]);

        assert!(has_section(&result.stdout, "skills"));
        assert!(has_section(&result.stdout, "mcps"));
        assert!(!has_section(&result.stdout, "packs"));
        assert!(!has_section(&result.stdout, "hooks"));
    }

    #[test]
    fn emits_every_namespace_as_a_json_key_whatever_was_asked_for() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "*", "--capability", "skill", "--json"]);
        let emitted = json_of(&result.stdout);
        let object = emitted.as_object().unwrap();

        assert_eq!(
            object.keys().collect::<Vec<_>>(),
            ["catalogs", "hooks", "mcps", "packs", "skills"]
        );
        assert!(!object["skills"].as_object().unwrap().is_empty());
        assert_eq!(object["packs"], json!({}));
        assert_eq!(object["mcps"], json!({}));
        assert_eq!(object["hooks"], json!({}));
    }

    #[test]
    fn limits_to_one_catalog_and_widens_when_catalog_is_repeated() {
        let fixture = Fixture::new();
        let second = "acme";

        fixture.write_colliding_catalog(second);
        fixture.write_catalog_order(&[second], &[]);

        // `company-context` is the name both catalogs provide, so it is the one that can tell a
        // filter that narrowed from a filter that did nothing.
        let one = fixture.cli(&[
            "search",
            CORE_SKILL,
            "--capability",
            "skill",
            "--catalog",
            second,
        ]);

        assert_eq!(
            rows_under(&one.stdout, "skills"),
            [format!("{CORE_SKILL}  {second}")]
        );
        // The header answers *where did I just look*, so it narrows with the filter.
        assert!(
            !one.stdout
                .contains(&format!("{CATALOG_NAME}  path:../catalog"))
        );

        let both = fixture.cli(&[
            "search",
            CORE_SKILL,
            "--capability",
            "skill",
            "--catalog",
            second,
            "--catalog",
            CATALOG_NAME,
        ]);

        assert_eq!(
            rows_under(&both.stdout, "skills"),
            [
                format!("{CORE_SKILL}  {second}"),
                format!("{CORE_SKILL}  {CATALOG_NAME}"),
            ]
        );
    }

    #[test]
    fn narrows_across_flags() {
        let fixture = Fixture::new();
        let second = "acme";

        fixture.write_colliding_catalog(second);
        fixture.write_catalog_order(&[second], &[]);

        // `jane-notes` exists only in the second catalog, so restricting to the first is a filter
        // the pattern alone would not have applied.
        let result = fixture.cli(&[
            "search",
            OWN_SKILL,
            "--capability",
            "skill",
            "--catalog",
            CATALOG_NAME,
        ]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(rows_under(&result.stdout, "skills"), ["(none)"]);
    }

    #[test]
    fn succeeds_with_an_empty_report_when_the_pattern_matches_nothing() {
        // A requirement reaching nothing is a config that will not do what it says, while a search
        // finding nothing is the answer to the search.
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "no-such-*"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(result.stderr, "");
        for title in ["packs", "skills", "mcps", "hooks"] {
            assert_eq!(rows_under(&result.stdout, title), ["(none)"], "{title}");
        }
    }

    #[test]
    fn exits_2_on_a_catalog_this_project_does_not_list_naming_what_it_does() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "*", "--catalog", "nope"]);

        assert_eq!(result.code, ExitCode::Config);
        assert_eq!(result.stdout, "");
        assert!(result.stderr.contains("no catalog named \"nope\""));
        assert!(
            result
                .stderr
                .contains(&format!("this project lists: {CATALOG_NAME}"))
        );
    }

    #[test]
    fn exits_2_on_a_capability_that_is_not_a_namespace() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search", "*", "--capability", "tag"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(result.stderr.contains("pack, skill, mcp, hook"));
    }

    #[test]
    fn requires_the_pattern_so_star_is_asked_for_rather_than_defaulted_to() {
        let fixture = Fixture::new();
        let result = fixture.cli(&["search"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(result.stderr.contains("missing required argument"));
    }
}

/// `hooks/<name>/hook.yml`: the third namespace a catalog distributes, and the one whose
/// declaration is not the whole truth about it.
///
/// A hook is a directory for the same reason a skill is (it may ship bytes), so it is found and
/// named exactly as a skill is. The half worth the module is what happens once a document says
/// `type: script`: the catalog is asked whether the file is really there, so a misspelled script
/// name is a refusal naming what the directory actually holds.
///
/// Every case writes the hook it is about into the fixture, beside the three the fixture ships, and
/// reads back only what it wrote.
mod catalog_hooks {
    use super::*;
    use crate::model::hook_entity::HookEvent;

    const HOOK_NAME: &str = "block-rm";
    const HOOK_DIR: &str = "hooks/block-rm";
    const HOOK_FILE: &str = "hooks/block-rm/hook.yml";

    /// The second catalog, for the one case about two of them providing one hook.
    const SECOND_CATALOG: &str = "personal";

    /// The hooks the fixture itself ships, which every case here writes beside.
    const FIXTURE_HOOKS: [&str; 3] = ["acme-standup", "guard-secrets", "session-notes"];

    /// A hook document, its `name` given separately so a caller writes only what the case is
    /// about.
    fn document(name: &str, lines: &[&str]) -> String {
        let mut all = vec![format!("name: {name}")];

        all.extend(lines.iter().map(|&line| line.to_owned()));
        all.push(String::new());
        all.join("\n")
    }

    /// Writes a hook into a catalog beside the fixture, so two catalogs can provide one name.
    fn write_hook_in(fixture: &Fixture, catalog: &str, name: &str, lines: &[&str]) {
        write_file(
            &fixture
                .root
                .join(catalog)
                .join("hooks")
                .join(name.replace('.', "/"))
                .join("hook.yml"),
            &document(name, lines),
        );
    }

    /// The hooks a case wrote, parsed: the fixture's own filtered out.
    fn hooks(fixture: &Fixture) -> Vec<CatalogHook> {
        fixture
            .parse()
            .unwrap()
            .hooks
            .into_iter()
            .filter(|hook| !FIXTURE_HOOKS.contains(&hook.name.as_str()))
            .collect()
    }

    #[test]
    fn reads_every_hook_directory_deriving_the_name_and_the_path_from_it() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            HOOK_FILE,
            &document(
                HOOK_NAME,
                &[
                    "description: Refuses a destructive rm before it runs",
                    "event: PreToolUse",
                    "matcher: Bash",
                    "type: command",
                    "command: npx block-rm",
                    "timeout: 30",
                    "expects:",
                    "  - env: BLOCK_RM_TOKEN",
                ],
            ),
        );

        assert_eq!(
            hooks(&fixture),
            [CatalogHook {
                name: HOOK_NAME.to_owned(),
                path: HOOK_DIR.to_owned(),
                description: Some("Refuses a destructive rm before it runs".to_owned()),
                event: HookEvent::PreToolUse,
                matcher: Some("Bash".to_owned()),
                r#type: HookType::Command,
                command: "npx block-rm".to_owned(),
                timeout: Some(30),
                expects: vec![Reference {
                    kind: ExpectationKind::Env,
                    name: "BLOCK_RM_TOKEN".to_owned(),
                }],
            }]
        );
    }

    #[test]
    fn joins_a_nested_hooks_path_segments_with_a_dot() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            "hooks/team/notify/hook.yml",
            &document(
                "team.notify",
                &["event: Stop", "type: command", "command: npx notify"],
            ),
        );

        let found = hooks(&fixture);

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "team.notify");
        assert_eq!(found[0].path, "hooks/team/notify");
    }

    #[test]
    fn accepts_a_declared_script_the_directory_holds_whether_bare_or_nested() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            HOOK_FILE,
            &document(
                HOOK_NAME,
                &["event: Stop", "type: script", "command: hook.sh"],
            ),
        );
        fixture.write_catalog_file(&format!("{HOOK_DIR}/hook.sh"), "#!/bin/sh\nexit 0\n");

        let found = hooks(&fixture);

        assert_eq!(
            (found[0].command.as_str(), found[0].r#type),
            ("hook.sh", HookType::Script)
        );

        fixture.write_catalog_file(
            HOOK_FILE,
            &document(
                HOOK_NAME,
                &[
                    "event: Stop",
                    "type: script",
                    "command: ./bin/hook --verbose",
                ],
            ),
        );
        fixture.write_catalog_file(&format!("{HOOK_DIR}/bin/hook"), "#!/bin/sh\nexit 0\n");

        // The program is the only token that can name a shipped file; its arguments are the
        // harness's.
        let found = hooks(&fixture);

        assert_eq!(
            (found[0].command.as_str(), found[0].r#type),
            ("./bin/hook --verbose", HookType::Script)
        );
    }

    #[test]
    fn takes_a_command_line_as_written_whatever_its_first_word_looks_like() {
        let fixture = Fixture::new();

        for command in [
            "npx prettier --write",
            "python3.11 check.py",
            "/usr/bin/say done",
        ] {
            fixture.write_catalog_file(
                HOOK_FILE,
                &document(
                    HOOK_NAME,
                    &[
                        "event: Stop",
                        "type: command",
                        &format!("command: {command}"),
                    ],
                ),
            );

            let found = hooks(&fixture);

            assert_eq!(
                (found[0].command.as_str(), found[0].r#type),
                (command, HookType::Command)
            );
        }
    }

    #[test]
    fn leaves_a_command_line_alone_even_when_the_directory_happens_to_hold_that_name() {
        // `type` is the whole answer, so a file sitting there is a coincidence rather than a
        // signal: this hook runs `hook.sh` off the PATH, and nothing is materialized for it.
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            HOOK_FILE,
            &document(
                HOOK_NAME,
                &["event: Stop", "type: command", "command: hook.sh"],
            ),
        );
        fixture.write_catalog_file(&format!("{HOOK_DIR}/hook.sh"), "#!/bin/sh\nexit 0\n");

        let found = hooks(&fixture);

        assert_eq!(
            (found[0].command.as_str(), found[0].r#type),
            ("hook.sh", HookType::Command)
        );
    }

    #[test]
    fn refuses_a_script_the_hooks_directory_does_not_hold_listing_what_it_does() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            HOOK_FILE,
            &document(
                HOOK_NAME,
                &["event: Stop", "type: script", "command: hook.sh"],
            ),
        );
        fixture.write_catalog_file(&format!("{HOOK_DIR}/hoook.sh"), "#!/bin/sh\nexit 0\n");
        fixture.write_catalog_file(&format!("{HOOK_DIR}/lib/helper.sh"), "#!/bin/sh\nexit 0\n");

        let error = fixture.rejection();

        assert_eq!(
            error.message,
            format!("hook \"{HOOK_NAME}\" ships no hook.sh ({HOOK_FILE} line 4)")
        );
        assert!(
            error
                .detail
                .contains(&format!("{HOOK_DIR} holds: hoook.sh, lib/helper.sh"))
        );
        assert!(
            error
                .detail
                .join("\n")
                .contains("say `type: command` instead")
        );
    }

    #[test]
    fn says_the_directory_holds_nothing_else_when_a_hook_ships_no_files_at_all() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            HOOK_FILE,
            &document(
                HOOK_NAME,
                &["event: Stop", "type: script", "command: hook.sh"],
            ),
        );

        assert!(
            fixture
                .rejection()
                .detail
                .contains(&format!("{HOOK_DIR} holds nothing but hook.yml"))
        );
    }

    #[test]
    fn refuses_a_hook_whose_name_disagrees_with_its_path() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            HOOK_FILE,
            &document(
                "wrong-name",
                &["event: Stop", "type: command", "command: npx x"],
            ),
        );

        let error = fixture.rejection();

        assert_eq!(
            error.message,
            format!("hook name \"wrong-name\" does not match its path ({HOOK_FILE} line 1)")
        );
        assert!(
            error
                .detail
                .join("\n")
                .contains(&format!("derives the name \"{HOOK_NAME}\""))
        );
    }

    #[test]
    fn refuses_a_hook_yml_that_is_in_no_hook_directory_at_all() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            "hooks/hook.yml",
            &document("x", &["event: Stop", "type: command", "command: npx x"]),
        );

        assert_eq!(
            fixture.rejection().message,
            "hooks/hook.yml is not inside a hook directory"
        );
    }

    #[test]
    fn carries_every_hook_entity_rejection_through() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            HOOK_FILE,
            &document(
                HOOK_NAME,
                &["event: OnTuesday", "type: command", "command: npx x"],
            ),
        );

        assert!(
            fixture
                .rejection()
                .message
                .contains("unknown hook event \"OnTuesday\"")
        );
    }

    #[test]
    fn shows_hooks_in_ambit_search_json_and_text() {
        let fixture = Fixture::new();

        fixture.write_catalog_file(
            HOOK_FILE,
            &document(
                HOOK_NAME,
                &[
                    "event: PreToolUse",
                    "matcher: Bash",
                    "type: script",
                    "command: hook.sh",
                ],
            ),
        );
        fixture.write_catalog_file(&format!("{HOOK_DIR}/hook.sh"), "#!/bin/sh\nexit 0\n");

        let emitted = json_of(&fixture.cli(&["search", "*", "--json"]).stdout);
        let hooks = emitted["hooks"].as_object().unwrap();

        assert_eq!(
            hooks[&format!("{CATALOG_NAME}/{HOOK_NAME}")],
            json!({
                "catalog": CATALOG_NAME,
                "type": "script",
                "command": "hook.sh",
                "event": "PreToolUse",
                "expects": [],
                "matcher": "Bash",
                "path": HOOK_DIR,
            })
        );

        // Each record is keyed by its address, since a name is not unique across catalogs.
        let mut expected: Vec<&str> = FIXTURE_HOOKS.to_vec();

        expected.push(HOOK_NAME);
        expected.sort_unstable();
        assert_eq!(
            hooks.keys().cloned().collect::<Vec<_>>(),
            expected
                .iter()
                .map(|name| format!("{CATALOG_NAME}/{name}"))
                .collect::<Vec<_>>()
        );

        // The row's fields rather than its padding, which widens with whatever else the catalog
        // holds.
        let stdout = fixture.cli(&["search", "*"]).stdout;
        let row = stdout
            .split('\n')
            .find(|line| line.trim_start().starts_with(&format!("{HOOK_NAME} ")))
            .unwrap();

        assert_eq!(
            row.split_whitespace().collect::<Vec<_>>().join(" "),
            format!("{HOOK_NAME} {CATALOG_NAME} PreToolUse hook.sh (shipped)")
        );
    }

    #[test]
    fn keeps_both_catalogs_copies_of_a_duplicate_hook_name_each_with_its_own_definition() {
        let fixture = Fixture::new();

        fixture.write_colliding_catalog(SECOND_CATALOG);
        write_hook_in(
            &fixture,
            "catalog",
            HOOK_NAME,
            &[
                "event: Stop",
                "type: command",
                "command: npx company-notify",
            ],
        );
        write_hook_in(
            &fixture,
            SECOND_CATALOG,
            HOOK_NAME,
            &["event: Stop", "type: command", "command: npx jane-notify"],
        );
        fixture.write_catalog_order(&[SECOND_CATALOG], &[]);

        let view = fixture.merged();

        // Two entries for the contested name, in catalog order, each carrying its own `command`.
        assert_eq!(
            view.hooks
                .iter()
                .filter(|hook| hook.name == HOOK_NAME)
                .map(|hook| (hook.catalog.as_str(), hook.command.as_str()))
                .collect::<Vec<_>>(),
            [
                (CATALOG_NAME, "npx company-notify"),
                (SECOND_CATALOG, "npx jane-notify"),
            ]
        );
    }
}

/// The two `search '*'` dumps from the command-surface suite; the rest of it is in
/// `src/cli/tests.rs`.
mod the_command_surface {
    use super::*;

    #[test]
    fn dumps_the_merged_catalog_under_ambit_search() {
        let fixture = Fixture::new();
        let dump = fixture.cli(&["search", "*"]);

        assert_eq!(dump.code, ExitCode::Success, "{}", dump.stderr);
        assert!(dump.stdout.contains(CATALOG_NAME));
    }

    #[test]
    fn emits_the_merged_catalog_as_json() {
        let fixture = Fixture::new();
        let dump = fixture.cli(&["search", "*", "--json"]);

        assert_eq!(dump.code, ExitCode::Success, "{}", dump.stderr);
        assert_eq!(json_of(&dump.stdout)["catalogs"], json!([CATALOG_NAME]));
    }
}

mod merging {
    use super::*;

    /// One catalog parsed straight from a directory, without resolving a source.
    fn catalog(name: &str, root: &Path) -> Catalog {
        parse_catalog_directory(
            name,
            "path:../catalog",
            root,
            None,
            &mut CatalogParseOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn tags_every_item_with_the_catalog_it_came_from() {
        let fixture = Fixture::new();
        let merged = merge_catalogs(&[catalog(CATALOG_NAME, &fixture.catalog_dir)]);

        assert_eq!(merged.catalogs, [CATALOG_NAME]);
        assert!(
            merged
                .skills
                .iter()
                .all(|item| item.catalog == CATALOG_NAME)
        );
        assert!(merged.mcps.iter().all(|item| item.catalog == CATALOG_NAME));
        assert!(
            merged
                .skills
                .iter()
                .all(|item| item.catalog_root == fixture.catalog_dir)
        );
    }

    #[test]
    fn tags_every_loaded_item_with_the_catalog_it_came_from() {
        let fixture = Fixture::new();
        let merged = fixture.merged();

        assert_eq!(merged.catalogs, [CATALOG_NAME]);
        assert!(
            merged
                .skills
                .iter()
                .all(|item| item.catalog == CATALOG_NAME)
        );
        assert!(merged.mcps.iter().all(|item| item.catalog == CATALOG_NAME));
    }

    #[test]
    fn keeps_every_catalogs_copy_of_a_duplicate_name_grouped_by_name_then_catalog() {
        let fixture = Fixture::new();
        let other = fixture.root.join("other");

        build_fixture_catalog(&other).unwrap();

        let merged = merge_catalogs(&[
            catalog(CATALOG_NAME, &fixture.catalog_dir),
            catalog("personal", &other),
        ]);

        assert_eq!(merged.catalogs, [CATALOG_NAME, "personal"]);
        // Two identical catalogs, so every name is provided twice and nothing is dropped.
        assert_eq!(merged.skills.len(), 8);
        assert_eq!(
            merged
                .skills
                .iter()
                .map(|skill| format!("{} {}", skill.name, skill.catalog))
                .collect::<Vec<_>>(),
            [
                "acme-brief company",
                "acme-brief personal",
                "code-review company",
                "code-review personal",
                "company-context company",
                "company-context personal",
                "design-tokens company",
                "design-tokens personal",
            ]
        );
    }

    #[test]
    fn rewrites_only_a_shipped_scripts_program_token() {
        let fixture = Fixture::new();
        let merged = merge_catalogs(&[catalog(CATALOG_NAME, &fixture.catalog_dir)]);
        let mut hook = merged
            .hooks
            .iter()
            .find(|hook| hook.name == "guard-secrets")
            .unwrap()
            .clone();

        assert_eq!(
            hook_command(&hook, ".claude/hooks"),
            ".claude/hooks/guard-secrets/guard.sh"
        );

        hook.command = "  ./guard.sh --strict \"a b\" ".to_owned();
        assert_eq!(
            hook_command(&hook, "$ROOT"),
            "$ROOT/guard-secrets/guard.sh --strict \"a b\""
        );

        hook.r#type = HookType::Command;
        assert_eq!(hook_command(&hook, "$ROOT"), hook.command);
    }

    #[test]
    fn spells_an_address_with_the_catalog_separator() {
        assert_eq!(qualified_name("company", "core.a"), "company/core.a");
        assert_eq!(skill_name_from_path("a/b/c"), "a.b.c");
    }
}

/// Several catalogs merge into one namespace per kind, and **every** copy of a name survives:
/// `catalogs:` order settles nothing, because there is no precedence left to establish.
///
/// A name two catalogs provide becomes a refusal only where a project selects both copies, and then
/// at resolve rather than at the merge: harness layout is flat, so the two would be installed at
/// one path.
///
/// The second catalog is written per test rather than added to the shared fixture.
mod multi_catalog_merge {
    use super::*;

    const SECOND: &str = "personal";
    const THIRD: &str = "backup";

    #[test]
    fn keeps_both_catalogs_copies_of_a_duplicate_name() {
        let fixture = Fixture::new();

        fixture.write_colliding_catalog(SECOND);
        fixture.write_catalog_order(&[SECOND], &[]);

        let view = fixture.merged();

        assert_eq!(view.catalogs, [CATALOG_NAME, SECOND]);
        assert_eq!(
            view.skills
                .iter()
                .filter(|skill| skill.name == CORE_SKILL)
                .map(|skill| skill.catalog.as_str())
                .collect::<Vec<_>>(),
            [CATALOG_NAME, SECOND]
        );
        assert_eq!(
            view.mcps
                .iter()
                .filter(|mcp| mcp.name == "linter")
                .map(|mcp| mcp.catalog.as_str())
                .collect::<Vec<_>>(),
            [CATALOG_NAME, SECOND]
        );
    }

    #[test]
    fn keeps_what_one_catalog_alone_provides_exactly_once() {
        let fixture = Fixture::new();

        fixture.write_colliding_catalog(SECOND);
        fixture.write_catalog_order(&[SECOND], &[]);

        let view = fixture.merged();

        assert_eq!(
            view.skills
                .iter()
                .filter(|skill| skill.name == OWN_SKILL)
                .map(|skill| skill.catalog.as_str())
                .collect::<Vec<_>>(),
            [SECOND]
        );
    }

    #[test]
    fn keeps_each_copys_own_definition_not_one_body_under_two_catalog_names() {
        // The transports differ, so this is the assertion that both bodies are in the merged view
        // rather than one of them twice.
        let fixture = Fixture::new();

        fixture.write_colliding_catalog(SECOND);
        fixture.write_catalog_order(&[SECOND], &[]);

        let dumped = json_of(&fixture.cli(&["search", "*", "--json"]).stdout);
        let mcps = &dumped["mcps"];

        assert_eq!(
            mcps[format!("{CATALOG_NAME}/linter")]["catalog"],
            CATALOG_NAME
        );
        assert_eq!(
            mcps[format!("{CATALOG_NAME}/linter")]["transport"]["kind"],
            "http"
        );
        assert_eq!(mcps[format!("{SECOND}/linter")]["catalog"], SECOND);
        assert_eq!(
            mcps[format!("{SECOND}/linter")]["transport"]["kind"],
            "stdio"
        );
        assert_eq!(
            mcps[format!("{SECOND}/linter")]["transport"]["command"],
            format!("{SECOND}-mcp")
        );
    }

    #[test]
    fn keeps_all_three_copies_when_three_catalogs_provide_one_name() {
        let fixture = Fixture::new();

        fixture.write_colliding_catalog(SECOND);
        fixture.write_colliding_catalog(THIRD);
        fixture.write_catalog_order(&[SECOND, THIRD], &[]);

        // In catalog-name order rather than config order: the merged view is sorted by name and
        // then catalog.
        assert_eq!(
            fixture
                .merged()
                .skills
                .iter()
                .filter(|skill| skill.name == CORE_SKILL)
                .map(|skill| skill.catalog.clone())
                .collect::<Vec<_>>(),
            [THIRD, CATALOG_NAME, SECOND]
        );
    }

    #[test]
    fn refuses_a_selection_that_reaches_both_copies_of_a_skill_naming_both_catalogs() {
        let fixture = Fixture::new();

        fixture.write_colliding_catalog(SECOND);
        fixture.write_catalog_order(
            &[SECOND],
            &[
                format!("  - {{ skill: \"{CATALOG_NAME}/{CORE_SKILL}\" }}"),
                format!("  - {{ skill: \"{SECOND}/{CORE_SKILL}\" }}"),
            ],
        );

        let result = fixture.cli(&["resolve"]);

        assert_eq!(result.code, ExitCode::Resolution);
        assert!(result.stderr.contains(&format!(
            "skill \"{CORE_SKILL}\" is selected from more than one catalog"
        )));
        assert!(
            result
                .stderr
                .contains(&format!("provided by: {CATALOG_NAME}, {SECOND}"))
        );
        assert!(result.stderr.contains(
            "a harness reads one entry per name, so both copies would be installed at the same path"
        ));
        assert!(result.stderr.contains(
            "select only one copy: narrow a `requires` pattern, or drop the entry that reaches the other catalog"
        ));
    }

    #[test]
    fn refuses_a_selected_mcp_server_two_catalogs_provide_as_it_does_a_skill() {
        let fixture = Fixture::new();

        fixture.write_colliding_catalog(SECOND);
        fixture.write_catalog_order(
            &[SECOND],
            &[
                format!("  - {{ mcp: \"{CATALOG_NAME}/linter\" }}"),
                format!("  - {{ mcp: \"{SECOND}/linter\" }}"),
            ],
        );

        let result = fixture.cli(&["resolve"]);

        assert_eq!(result.code, ExitCode::Resolution);
        assert!(
            result
                .stderr
                .contains("MCP server \"linter\" is selected from more than one catalog")
        );
    }

    #[test]
    fn resolves_normally_with_no_whose_copy_column_when_one_copy_is_selected() {
        let fixture = Fixture::new();

        fixture.write_colliding_catalog(SECOND);
        fixture.write_catalog_order(
            &[SECOND],
            &[format!("  - {{ skill: \"{SECOND}/{OWN_SKILL}\" }}")],
        );

        let result = fixture.cli(&["resolve", "--explain"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(
            result.stdout,
            [
                "packs (0)".to_owned(),
                "  (none)".to_owned(),
                String::new(),
                "skills (1)".to_owned(),
                format!("  {OWN_SKILL}  {SECOND}  skill:{SECOND}/{OWN_SKILL}"),
                String::new(),
                "mcps (0)".to_owned(),
                "  (none)".to_owned(),
                String::new(),
                "hooks (0)".to_owned(),
                "  (none)".to_owned(),
                String::new(),
                "expects (0)".to_owned(),
                "  (none)".to_owned(),
                String::new(),
            ]
            .join("\n")
        );
    }

    #[test]
    fn carries_nothing_about_other_copies_into_explain_json() {
        let fixture = Fixture::new();

        fixture.write_colliding_catalog(SECOND);
        fixture.write_catalog_order(&[SECOND], &[requires_entry("person.jane", SECOND)]);

        let explained = json_of(&fixture.cli(&["resolve", "--explain", "--json"]).stdout);
        let skills = explained["skills"].as_object().unwrap();

        // Keyed by name, because a bundle holds one item per name, and carrying only what the
        // bundle knows: where it came from, and why.
        assert_eq!(skills.keys().collect::<Vec<_>>(), [OWN_SKILL]);
        assert_eq!(
            skills[OWN_SKILL],
            json!({
                "catalog": SECOND,
                "path": format!("skills/{OWN_SKILL}"),
                "reason": "required-by:pack:person.jane",
            })
        );
    }
}
