//! Browsing the fixture catalog from disk: per-catalog loading, cache-only misses, item details and
//! the read-only document views.

use std::fs;
use std::path::PathBuf;

use super::*;
use crate::harness::adapter::HookSkipReason;
use crate::model::config::parse_project_config;
use crate::model::pattern::format_entry;
use crate::test_support::fixture_catalog::{
    FixtureGitCatalog, build_fixture_catalog, build_fixture_git_catalog,
};
use crate::test_support::{tempdir, test_env};

/// A project directory beside a `path:` fixture catalog and a git fixture catalog, with a cache no
/// test has fetched into.
struct Setup {
    _dir: tempfile::TempDir,
    project: PathBuf,
    env: Env,
    git: FixtureGitCatalog,
}

impl Setup {
    fn new() -> Self {
        let dir = tempdir();
        let root = dir.path().to_path_buf();
        let project = root.join("project");

        fs::create_dir_all(&project).unwrap();
        build_fixture_catalog(&root.join("catalog")).unwrap();

        Self {
            env: test_env(&root),
            git: build_fixture_git_catalog(&root.join("remote")).unwrap(),
            project,
            _dir: dir,
        }
    }

    /// A config listing `catalogs` (YAML lines) and selecting `requires` (YAML lines).
    fn config(catalogs: &[String], harnesses: &str, requires: &[&str]) -> ProjectConfig {
        let requires = if requires.is_empty() {
            " []".to_owned()
        } else {
            requires
                .iter()
                .map(|line| format!("\n  - {line}"))
                .collect::<Vec<_>>()
                .concat()
        };

        parse_project_config(
            &format!(
                "version: 1\nharnesses: [{harnesses}]\ncatalogs:\n{}\nrequires:{requires}\n",
                catalogs.join("\n")
            ),
            "ambit.yml",
        )
        .unwrap()
    }

    fn local() -> String {
        "  - name: company\n    source: path:../catalog".to_owned()
    }

    fn remote(&self) -> String {
        format!("  - name: remote\n    source: {}", self.git.url)
    }

    fn load(&self, config: ProjectConfig, policy: FetchPolicy) -> LoadedSetup {
        load_setup(&self.project, &self.env, config, policy).unwrap()
    }

    /// The local catalog alone, selecting `requires`.
    fn local_setup(&self, requires: &[&str]) -> LoadedSetup {
        self.load(
            Self::config(&[Self::local()], "claude", requires),
            FetchPolicy::CacheOnly,
        )
    }
}

fn find<'i>(items: &'i [BrowseItem], kind: ItemKind, catalog: &str, name: &str) -> &'i BrowseItem {
    items
        .iter()
        .find(|item| item.kind == kind && item.catalog == catalog && item.name == name)
        .expect("the item is listed")
}

fn names(items: &[BundleItem]) -> Vec<String> {
    items
        .iter()
        .map(|item| format!("{}:{}", item.kind, item.name))
        .collect()
}

mod loading {
    use super::*;

    #[test]
    fn reports_an_uncached_git_catalog_without_hiding_the_others() {
        let t = Setup::new();
        let loaded = t.load(
            Setup::config(
                &[Setup::local(), t.remote()],
                "claude",
                &["skill: \"remote/code-review\""],
            ),
            FetchPolicy::CacheOnly,
        );

        assert!(
            matches!(&loaded.catalogs[0], CatalogLoad::Loaded(catalog) if catalog.name == "company")
        );
        let CatalogLoad::NotCached { name, error } = &loaded.catalogs[1] else {
            panic!("expected the git catalog to be reported as not cached");
        };
        assert_eq!(name, "remote");
        assert_eq!(error.code, ExitCode::Network);
        assert_eq!(loaded.merged.catalogs, vec!["company", "remote"]);
        assert!(
            loaded
                .merged
                .skills
                .iter()
                .all(|skill| skill.catalog == "company")
        );

        // An entry for a catalog that did not load is not judged.
        let (items, problems) = browse(&loaded);

        assert!(problems.is_empty(), "{problems:?}");
        assert!(items.iter().all(|item| !item.selected));
    }

    #[test]
    fn fetches_what_the_cache_is_missing_when_allowed() {
        let t = Setup::new();
        let config = Setup::config(&[t.remote()], "claude", &[]);
        let loaded = t.load(config.clone(), FetchPolicy::FetchMissing);

        let CatalogLoad::Loaded(catalog) = &loaded.catalogs[0] else {
            panic!("expected the git catalog to load: {:?}", loaded.catalogs[0]);
        };
        assert_eq!(catalog.commit.as_deref(), Some(t.git.commit.as_str()));

        // Fetched once, it answers from the cache from then on.
        let again = t.load(config, FetchPolicy::CacheOnly);

        assert!(matches!(&again.catalogs[0], CatalogLoad::Loaded(_)));
    }

    #[test]
    fn reports_a_missing_local_catalog_as_failed() {
        let t = Setup::new();
        let loaded = t.load(
            Setup::config(
                &["  - name: gone\n    source: path:../nowhere".to_owned()],
                "claude",
                &[],
            ),
            FetchPolicy::CacheOnly,
        );

        let CatalogLoad::Failed { name, error } = &loaded.catalogs[0] else {
            panic!("expected the catalog to fail: {:?}", loaded.catalogs[0]);
        };
        assert_eq!(name, "gone");
        assert_eq!(error.code, ExitCode::Config);
    }

    #[test]
    fn reuses_the_load_for_a_draft_with_the_same_catalogs() {
        let t = Setup::new();
        let loaded = t.local_setup(&[]);
        let draft = Setup::config(&[Setup::local()], "claude", &["pack: \"company/core\""]);
        let reused = reconfigure(&loaded, draft.clone()).expect("same catalogs");

        assert_eq!(reused.config, draft);
        assert_eq!(reused.merged, loaded.merged);

        let moved = Setup::config(&[t.remote()], "claude", &[]);

        assert!(reconfigure(&loaded, moved).is_none());
    }
}

mod listing {
    use super::*;

    #[test]
    fn lists_every_item_selected_or_not() {
        let t = Setup::new();
        let (items, problems) = browse(&t.local_setup(&["pack: \"company/core\""]));

        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(
            items
                .iter()
                .map(|item| format!(
                    "{}:{}{}",
                    item.kind,
                    item.name,
                    if item.selected { " *" } else { "" }
                ))
                .collect::<Vec<_>>(),
            vec![
                "pack:core *",
                "pack:function.engineering",
                "pack:function.engineering.frontend",
                "pack:project.acme",
                "skill:acme-brief",
                "skill:code-review",
                "skill:company-context *",
                "skill:design-tokens",
                "mcp:fixture",
                "mcp:linter",
                "hook:acme-standup",
                "hook:guard-secrets",
                "hook:session-notes *",
            ]
        );
        assert!(
            items
                .iter()
                .filter(|item| !item.selected)
                .all(|item| item.routes.is_empty())
        );
    }

    #[test]
    fn shows_every_route_of_a_selected_item() {
        let t = Setup::new();
        let (items, _) = browse(&t.local_setup(&[
            "pack: \"company/core\"",
            "skill: \"company/acme-brief\"",
            "skill: \"company/company-*\"",
        ]));
        let routes: Vec<String> = find(&items, ItemKind::Skill, "company", "company-context")
            .routes
            .iter()
            .map(|route| match route {
                Route::Entry { entry } => format_entry(entry),
                Route::RequiredBy { requirer, .. } => {
                    format!("required-by:{}:{}", requirer.kind, requirer.name)
                }
            })
            .collect();

        assert_eq!(
            routes,
            vec![
                "skill:company/company-*",
                "required-by:pack:core",
                "required-by:skill:acme-brief",
            ]
        );
    }

    #[test]
    fn tells_two_catalogs_copies_of_one_name_apart() {
        let t = Setup::new();
        let config = Setup::config(
            &[
                Setup::local(),
                "  - name: second\n    source: path:../catalog".to_owned(),
            ],
            "claude",
            &["skill: \"second/code-review\""],
        );
        let (items, problems) = browse(&t.load(config, FetchPolicy::CacheOnly));

        assert!(problems.is_empty(), "{problems:?}");
        assert!(!find(&items, ItemKind::Skill, "company", "code-review").selected);
        assert!(find(&items, ItemKind::Skill, "second", "code-review").selected);
    }

    #[test]
    fn reports_unmatched_entries_and_keeps_the_rest_selected() {
        let t = Setup::new();
        let (items, problems) = browse(&t.local_setup(&[
            "pack: \"company/core\"",
            "skill: \"company/zzz.*\"",
            "hook: \"company/nope\"",
        ]));

        assert_eq!(problems.len(), 2);
        assert!(
            problems
                .iter()
                .all(|error| error.code == ExitCode::Resolution)
        );
        assert!(problems[0].message.contains("hook:company/nope"));
        assert!(find(&items, ItemKind::Pack, "company", "core").selected);
    }

    #[test]
    fn carries_each_kind_s_details_as_data() {
        let t = Setup::new();
        let loaded = t.load(
            Setup::config(&[Setup::local()], "claude, opencode", &[]),
            FetchPolicy::CacheOnly,
        );
        let (items, _) = browse(&loaded);

        let skill = find(&items, ItemKind::Skill, "company", "acme-brief");
        assert_eq!(
            skill.description.as_deref(),
            Some("The Acme engagement brief — remit, contacts, and conventions.")
        );
        assert_eq!(
            skill.requires.iter().map(format_entry).collect::<Vec<_>>(),
            vec!["skill:company-context", "mcp:fixture", "hook:acme-standup"]
        );
        assert_eq!(
            skill.detail,
            ItemDetail::Skill {
                path: "skills/acme-brief".to_owned()
            }
        );

        let mcp = find(&items, ItemKind::Mcp, "company", "fixture");
        let ItemDetail::Mcp {
            transport: McpTransport::Stdio(stdio),
            ..
        } = &mcp.detail
        else {
            panic!("expected a stdio server: {:?}", mcp.detail);
        };
        assert_eq!(stdio.command, "npx");
        assert_eq!(stdio.args, vec!["-y", "@acme/fixture-mcp"]);
        assert_eq!(mcp.expects[0].name, "FIXTURE_API_KEY");

        let hook = find(&items, ItemKind::Hook, "company", "guard-secrets");
        assert_eq!(
            hook.detail,
            ItemDetail::Hook {
                path: "hooks/guard-secrets".to_owned(),
                event: HookEvent::PreToolUse,
                matcher: Some("Bash".to_owned()),
                hook_type: HookType::Script,
                command: "guard.sh".to_owned(),
                timeout: Some(10),
            }
        );
        // opencode has no hook mechanism; claude expresses every event.
        assert_eq!(
            hook.limitations
                .iter()
                .map(|skipped| (skipped.harness.as_str(), skipped.reason))
                .collect::<Vec<_>>(),
            vec![("opencode", HookSkipReason::NoMechanism)]
        );
        assert_eq!(mcp.limitations, Vec::new());
    }
}

mod documents {
    use super::*;

    #[test]
    fn reads_a_skill_document_without_its_delimiters() {
        let t = Setup::new();
        let document = skill_document(&t.local_setup(&[]), "company", "company-context").unwrap();

        assert_eq!(document.path, "skills/company-context/SKILL.md");
        assert!(document.frontmatter.starts_with("name: company-context\n"));
        assert!(!document.frontmatter.contains("---"));
        assert!(document.body.starts_with("\n# Acme company context\n"));
    }

    #[test]
    fn refuses_a_skill_the_catalog_does_not_hold() {
        let t = Setup::new();
        let error =
            skill_document(&t.local_setup(&[]), "company", "nope").expect_err("no such skill");

        assert_eq!(error.code, ExitCode::Resolution);
    }

    #[test]
    fn resolves_a_pack_s_contents_as_if_it_were_selected_alone() {
        let t = Setup::new();
        let bundle = pack_contents(&t.local_setup(&[]), "company", "function.engineering").unwrap();
        let shown = |names: Vec<&String>| names.into_iter().cloned().collect::<Vec<_>>();

        assert_eq!(
            shown(bundle.packs.iter().map(|pack| &pack.name).collect()),
            vec!["core", "function.engineering"]
        );
        assert_eq!(
            shown(bundle.skills.iter().map(|skill| &skill.name).collect()),
            vec!["code-review", "company-context"]
        );
        assert_eq!(
            shown(bundle.mcps.iter().map(|mcp| &mcp.name).collect()),
            vec!["linter"]
        );
        assert_eq!(
            shown(bundle.hooks.iter().map(|hook| &hook.name).collect()),
            vec!["guard-secrets", "session-notes"]
        );
    }
}

mod rules {
    use super::*;
    use crate::model::pattern::{Addressing, parse_address};

    fn rule(kind: ItemKind, address: &str) -> PatternEntry {
        parse_address(kind, address, Addressing::Qualified).unwrap()
    }

    #[test]
    fn previews_the_current_matches_without_the_closure() {
        let t = Setup::new();
        let loaded = t.local_setup(&[]);

        assert_eq!(
            names(&rule_matches(&loaded, &rule(ItemKind::Pack, "company/function.*")).unwrap()),
            vec![
                "pack:function.engineering",
                "pack:function.engineering.frontend"
            ]
        );
        assert_eq!(
            names(&rule_matches(&loaded, &rule(ItemKind::Skill, "company/*-context")).unwrap()),
            vec!["skill:company-context"]
        );
    }

    #[test]
    fn refuses_a_rule_that_matches_nothing() {
        let t = Setup::new();
        let error = rule_matches(&t.local_setup(&[]), &rule(ItemKind::Mcp, "company/zzz.*"))
            .expect_err("nothing matches");

        assert_eq!(error.code, ExitCode::Resolution);
        assert!(error.message.contains("mcp:company/zzz.*"));
    }

    #[test]
    fn passes_on_the_load_error_of_a_catalog_that_did_not_load() {
        let t = Setup::new();
        let loaded = t.load(
            Setup::config(&[Setup::local(), t.remote()], "claude", &[]),
            FetchPolicy::CacheOnly,
        );
        let error =
            rule_matches(&loaded, &rule(ItemKind::Skill, "remote/*")).expect_err("not cached");

        assert_eq!(error.code, ExitCode::Network);
    }
}
