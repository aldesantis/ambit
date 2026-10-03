//! The browse exports against the fixture catalog, as a local folder and as a local bare git
//! repository.

#[path = "../../../ambit-core/tests/support/fixture_catalog.rs"]
mod fixture_catalog;

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;

use self::fixture_catalog::{FixtureGitCatalog, build_fixture_catalog, build_fixture_git_catalog};
use super::*;
use crate::engine::{Engine, EngineConfig};
use crate::errors::NetworkKind;

/// A project folder beside the fixture catalog and its git twin, with an empty cache.
struct Setup {
    _dir: TempDir,
    project: PathBuf,
    env: HashMap<String, String>,
    git: FixtureGitCatalog,
}

impl Setup {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        let project = root.join("project");

        fs::create_dir_all(&project).unwrap();
        build_fixture_catalog(&root.join("catalog")).unwrap();

        let mut env = HashMap::from([
            ("HOME".to_owned(), root.join("home").display().to_string()),
            (
                "XDG_CACHE_HOME".to_owned(),
                root.join("cache").display().to_string(),
            ),
        ]);

        if let Some(path) = path_var() {
            env.insert("PATH".to_owned(), path);
        }

        Self {
            git: build_fixture_git_catalog(&root.join("remote")).unwrap(),
            env,
            project,
            _dir: dir,
        }
    }

    /// A session on the project, with `requires` (YAML lines) saved against the local catalog.
    fn session(&self, requires: &[&str]) -> Arc<SetupSession> {
        fs::write(self.project.join("ambit.yml"), config(&[LOCAL], requires)).unwrap();
        self.open()
    }

    fn open(&self) -> Arc<SetupSession> {
        Engine::new(EngineConfig {
            env: self.env.clone(),
        })
        .open_setup(self.project.display().to_string())
    }

    fn remote(&self) -> String {
        format!("  - name: remote\n    source: {}", self.git.url)
    }
}

/// The real `PATH`, so git is found. Reading the process environment is safe in parallel tests.
#[allow(clippy::disallowed_methods)]
fn path_var() -> Option<String> {
    std::env::var("PATH").ok()
}

const LOCAL: &str = "  - name: company\n    source: path:../catalog";

fn config(catalogs: &[&str], requires: &[&str]) -> String {
    let requires: String = if requires.is_empty() {
        " []".to_owned()
    } else {
        requires.iter().fold(String::new(), |mut text, line| {
            text.push_str("\n  - ");
            text.push_str(line);
            text
        })
    };

    format!(
        "version: 1\nharnesses: [claude, opencode]\ncatalogs:\n{}\nrequires:{requires}\n",
        catalogs.join("\n")
    )
}

fn find<'i>(result: &'i BrowseResult, kind: ItemKind, name: &str) -> &'i BrowseItem {
    result
        .items
        .iter()
        .find(|item| item.kind == kind && item.name == name)
        .expect("the item is listed")
}

fn item(kind: ItemKind, name: &str) -> ItemRef {
    ItemRef {
        kind,
        catalog: "company".to_owned(),
        name: name.to_owned(),
    }
}

fn entry(kind: ItemKind, pattern: &str) -> SelectionEntry {
    SelectionEntry {
        kind,
        catalog: Some("company".to_owned()),
        pattern: pattern.to_owned(),
        is_rule: pattern.contains('*'),
    }
}

fn names(items: &[ItemRef]) -> Vec<String> {
    items
        .iter()
        .map(|item| format!("{:?}:{}", item.kind, item.name))
        .collect()
}

mod loading {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn reports_each_catalog_s_state() {
        let t = Setup::new();
        let draft = config(
            &[
                LOCAL,
                &t.remote(),
                "  - name: gone\n    source: path:../nowhere",
            ],
            &[],
        );
        let session = t.open();

        let state = session
            .load_catalogs(Some(draft), FetchPolicy::CacheOnly)
            .unwrap();

        assert_eq!(
            state.catalogs[0],
            CatalogAvailability {
                name: "company".to_owned(),
                state: CatalogLoadState::Loaded {
                    commit: None,
                    local: true
                },
            }
        );
        assert!(matches!(
            &state.catalogs[1].state,
            CatalogLoadState::NotCached {
                error: EngineError::Network {
                    kind: NetworkKind::NotCached,
                    ..
                }
            }
        ));
        assert!(matches!(
            &state.catalogs[2].state,
            CatalogLoadState::Failed {
                error: EngineError::Config { .. }
            }
        ));
    }

    #[test]
    fn fetches_what_the_cache_is_missing_when_allowed() {
        let t = Setup::new();
        let draft = config(&[&t.remote()], &["skill: \"remote/code-review\""]);
        let session = t.open();

        let state = session
            .load_catalogs(Some(draft.clone()), FetchPolicy::FetchMissing)
            .unwrap();

        assert_eq!(
            state.catalogs[0].state,
            CatalogLoadState::Loaded {
                commit: Some(t.git.commit.clone()),
                local: false
            }
        );
        let result = session.browse(Some(draft)).unwrap();
        assert!(find(&result, ItemKind::Skill, "code-review").selected);
    }

    #[test]
    fn a_missing_config_is_a_config_error() {
        let t = Setup::new();

        assert!(matches!(
            t.open().browse(None),
            Err(EngineError::Config { .. })
        ));
    }

    #[test]
    fn a_malformed_draft_names_its_line() {
        let t = Setup::new();
        let session = t.session(&[]);

        let error = session
            .browse(Some("version: 1\nbogus: true\n".to_owned()))
            .expect_err("the draft is invalid");

        assert!(matches!(
            error,
            EngineError::Config { line: Some(2), path: Some(ref path), .. } if path == "ambit.yml"
        ));
    }
}

mod listing {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn maps_every_kind_of_reason() {
        let t = Setup::new();
        let session = t.session(&[
            "pack: \"company/core\"",
            "skill: \"company/acme-brief\"",
            "skill: \"company/company-*\"",
        ]);

        let result = session.browse(None).unwrap();
        let context = find(&result, ItemKind::Skill, "company-context");

        assert_eq!(result.problems, Vec::new());
        assert_eq!(
            context.reasons,
            vec![
                SelectionReason::Rule {
                    entry: entry(ItemKind::Skill, "company-*")
                },
                SelectionReason::Pack {
                    pack: item(ItemKind::Pack, "core"),
                    chain: vec![
                        ChainLink {
                            item: item(ItemKind::Pack, "core"),
                            reason: ChainReason::Entry {
                                entry: entry(ItemKind::Pack, "core")
                            },
                        },
                        ChainLink {
                            item: item(ItemKind::Skill, "company-context"),
                            reason: ChainReason::RequiredBy {
                                requirer: item(ItemKind::Pack, "core")
                            },
                        },
                    ],
                },
                SelectionReason::Dependency {
                    requirer: item(ItemKind::Skill, "acme-brief"),
                    chain: vec![
                        ChainLink {
                            item: item(ItemKind::Skill, "acme-brief"),
                            reason: ChainReason::Entry {
                                entry: entry(ItemKind::Skill, "acme-brief")
                            },
                        },
                        ChainLink {
                            item: item(ItemKind::Skill, "company-context"),
                            reason: ChainReason::RequiredBy {
                                requirer: item(ItemKind::Skill, "acme-brief")
                            },
                        },
                    ],
                },
            ]
        );
        assert_eq!(
            find(&result, ItemKind::Skill, "acme-brief").reasons,
            vec![SelectionReason::Direct {
                entry: entry(ItemKind::Skill, "acme-brief")
            }]
        );
        assert!(!find(&result, ItemKind::Skill, "code-review").selected);
        assert_eq!(
            find(&result, ItemKind::Skill, "code-review").reasons,
            Vec::new()
        );
    }

    #[test]
    fn carries_details_as_data() {
        let t = Setup::new();
        let result = t.session(&[]).browse(None).unwrap();

        let brief = find(&result, ItemKind::Skill, "acme-brief");
        assert_eq!(
            brief.requires,
            vec![
                SelectionEntry {
                    kind: ItemKind::Skill,
                    catalog: None,
                    pattern: "company-context".to_owned(),
                    is_rule: false,
                },
                SelectionEntry {
                    kind: ItemKind::Mcp,
                    catalog: None,
                    pattern: "fixture".to_owned(),
                    is_rule: false,
                },
                SelectionEntry {
                    kind: ItemKind::Hook,
                    catalog: None,
                    pattern: "acme-standup".to_owned(),
                    is_rule: false,
                },
            ]
        );

        let fixture = find(&result, ItemKind::Mcp, "fixture");
        assert_eq!(
            fixture.detail,
            ItemDetail::Mcp {
                file: "mcps/fixture.yml".to_owned(),
                transport: McpTransport::Stdio {
                    command: "npx".to_owned(),
                    args: vec!["-y".to_owned(), "@acme/fixture-mcp".to_owned()],
                    env: Vec::new(),
                },
            }
        );
        assert_eq!(
            fixture.prerequisites,
            vec![Prerequisite::EnvironmentVariable {
                name: "FIXTURE_API_KEY".to_owned()
            }]
        );

        let linter = find(&result, ItemKind::Mcp, "linter");
        assert!(matches!(
            &linter.detail,
            ItemDetail::Mcp {
                transport: McpTransport::Http { url, bearer_token_env_var: Some(token), .. },
                ..
            } if url == "https://mcp.invalid/fixture" && token == "LINTER_API_KEY"
        ));

        let guard = find(&result, ItemKind::Hook, "guard-secrets");
        assert_eq!(
            guard.detail,
            ItemDetail::Hook {
                path: "hooks/guard-secrets".to_owned(),
                event: "PreToolUse".to_owned(),
                matcher: Some("Bash".to_owned()),
                hook_type: HookType::Script,
                command: "guard.sh".to_owned(),
                timeout_seconds: Some(10),
            }
        );
        assert_eq!(
            guard.limitations,
            vec![ToolLimitation {
                tool: "opencode".to_owned(),
                reason: LimitationReason::NoHooks,
                message: "opencode has no declarative hook mechanism".to_owned(),
            }]
        );
    }

    #[test]
    fn answers_for_a_draft_without_touching_the_saved_config() {
        let t = Setup::new();
        let session = t.session(&[]);
        let draft = config(&[LOCAL], &["pack: \"company/core\""]);

        let drafted = session.browse(Some(draft)).unwrap();
        let saved = session.browse(None).unwrap();

        assert!(find(&drafted, ItemKind::Pack, "core").selected);
        assert!(!find(&saved, ItemKind::Pack, "core").selected);
    }

    #[test]
    fn reports_unmatched_entries() {
        let t = Setup::new();
        let session = t.session(&["pack: \"company/core\"", "skill: \"company/zzz.*\""]);

        let result = session.browse(None).unwrap();
        let unmatched = session.unmatched_entries(None).unwrap();

        assert_eq!(result.problems.len(), 1);
        assert!(find(&result, ItemKind::Pack, "core").selected);
        assert_eq!(unmatched.len(), 1);
        assert_eq!(unmatched[0].entry, entry(ItemKind::Skill, "zzz.*"));
        assert!(matches!(unmatched[0].error, EngineError::Resolution { .. }));
    }

    #[test]
    fn does_not_judge_entries_for_a_catalog_that_did_not_load() {
        let t = Setup::new();
        let draft = config(&[LOCAL, &t.remote()], &["skill: \"remote/code-review\""]);

        let session = t.session(&[]);

        assert_eq!(
            session.browse(Some(draft.clone())).unwrap().problems,
            Vec::new()
        );
        assert_eq!(session.unmatched_entries(Some(draft)).unwrap(), Vec::new());
    }
}

mod documents {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn reads_a_skill_document() {
        let t = Setup::new();
        let document = t
            .session(&[])
            .skill_document("company", "company-context")
            .unwrap();

        assert_eq!(document.path, "skills/company-context/SKILL.md");
        assert!(document.frontmatter.starts_with("name: company-context\n"));
        assert!(document.body.starts_with("\n# Acme company context\n"));
    }

    #[test]
    fn refuses_an_unknown_skill() {
        let t = Setup::new();

        assert!(matches!(
            t.session(&[]).skill_document("company", "nope"),
            Err(EngineError::Resolution { .. })
        ));
    }

    #[test]
    fn lists_a_pack_s_contents() {
        let t = Setup::new();
        let contents = t
            .session(&[])
            .pack_contents("company", "function.engineering")
            .unwrap();

        assert_eq!(
            names(&contents),
            vec![
                "Pack:core",
                "Pack:function.engineering",
                "Skill:code-review",
                "Skill:company-context",
                "Mcp:linter",
                "Hook:guard-secrets",
                "Hook:session-notes",
            ]
        );
    }
}

mod rules {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn previews_the_current_matches() {
        let t = Setup::new();
        let matches = t
            .session(&[])
            .preview_rule(None, "company", ItemKind::Pack, "function.*")
            .unwrap();

        assert_eq!(
            names(&matches),
            vec![
                "Pack:function.engineering",
                "Pack:function.engineering.frontend"
            ]
        );
    }

    #[test]
    fn rejects_a_pattern_the_grammar_refuses() {
        let t = Setup::new();

        assert!(matches!(
            t.session(&[])
                .preview_rule(None, "company", ItemKind::Skill, "a/b"),
            Err(EngineError::Config { .. })
        ));
    }

    #[test]
    fn rejects_a_rule_that_matches_nothing() {
        let t = Setup::new();

        assert!(matches!(
            t.session(&[])
                .preview_rule(None, "company", ItemKind::Mcp, "zzz.*"),
            Err(EngineError::Resolution { .. })
        ));
    }
}

mod removal {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn names_every_entry_sustaining_an_item() {
        let t = Setup::new();
        let session = t.session(&["pack: \"company/core\"", "skill: \"company/company-*\""]);

        let impact = session
            .removal_impact(None, item(ItemKind::Skill, "company-context"))
            .unwrap();

        assert_eq!(
            impact
                .sustaining
                .iter()
                .map(|sustained| sustained.entry.clone())
                .collect::<Vec<_>>(),
            vec![
                entry(ItemKind::Pack, "core"),
                entry(ItemKind::Skill, "company-*")
            ]
        );
        // Either entry alone still selects it through the other.
        assert!(impact.effects.iter().all(
            |effect| effect.entry == entry(ItemKind::Pack, "core") || effect.removes.is_empty()
        ));
        assert_eq!(
            names(&impact.effects[0].removes),
            vec!["Pack:core", "Hook:session-notes"]
        );
        assert_eq!(
            names(&impact.removed),
            vec!["Pack:core", "Skill:company-context", "Hook:session-notes"]
        );
    }

    #[test]
    fn an_unselected_item_needs_nothing_removed() {
        let t = Setup::new();
        let impact = t
            .session(&[])
            .removal_impact(None, item(ItemKind::Skill, "code-review"))
            .unwrap();

        assert_eq!(impact.sustaining, Vec::new());
        assert_eq!(impact.removed, Vec::new());
    }

    #[test]
    fn answers_for_a_draft() {
        let t = Setup::new();
        let session = t.session(&[]);
        let draft = config(&[LOCAL], &["skill: \"company/acme-brief\""]);

        let impact = session
            .removal_impact(Some(draft), item(ItemKind::Mcp, "fixture"))
            .unwrap();

        assert_eq!(
            impact.sustaining,
            vec![SustainingEntry {
                entry: entry(ItemKind::Skill, "acme-brief"),
                chain: vec![
                    ChainLink {
                        item: item(ItemKind::Skill, "acme-brief"),
                        reason: ChainReason::Entry {
                            entry: entry(ItemKind::Skill, "acme-brief")
                        },
                    },
                    ChainLink {
                        item: item(ItemKind::Mcp, "fixture"),
                        reason: ChainReason::RequiredBy {
                            requirer: item(ItemKind::Skill, "acme-brief")
                        },
                    },
                ],
            }]
        );
        assert_eq!(
            names(&impact.removed),
            vec![
                "Skill:acme-brief",
                "Skill:company-context",
                "Mcp:fixture",
                "Hook:acme-standup"
            ]
        );
    }
}
