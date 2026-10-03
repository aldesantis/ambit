use std::fs;

use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::*;
use crate::records::{CatalogEntry, ItemKind, SelectionEntry, SourceKind};

const VALID: &str = "version: 1
harnesses: [claude, codex]
catalogs:
  - name: company
    source: path:../catalog
requires:
  - skill: \"company/core.*\"
  - pack: \"company/base\"
";

fn engine() -> Arc<Engine> {
    Engine::new(EngineConfig {
        env: HashMap::from([("HOME".to_owned(), "/nonexistent".to_owned())]),
    })
}

fn snapshot(root: &TempDir) -> SetupSnapshot {
    engine()
        .open_setup(root.path().display().to_string())
        .snapshot()
        .unwrap()
}

fn write(root: &TempDir, name: &str, text: &str) -> String {
    let path = root.path().join(name);

    fs::write(&path, text).unwrap();
    path.display().to_string()
}

#[test]
fn a_root_without_a_config_is_missing() {
    let root = TempDir::new().unwrap();
    let snapshot = snapshot(&root);

    assert_eq!(snapshot.root, root.path().display().to_string());
    assert_eq!(snapshot.config, ConfigState::Missing);
}

#[test]
fn a_valid_config_carries_its_text_and_summary() {
    let root = TempDir::new().unwrap();
    let path = write(&root, "ambit.yml", VALID);

    assert_eq!(
        snapshot(&root).config,
        ConfigState::Valid {
            path,
            file_name: "ambit.yml".to_owned(),
            text: VALID.to_owned(),
            summary: ConfigSummary {
                harnesses: vec!["claude".to_owned(), "codex".to_owned()],
                catalogs: vec![CatalogEntry {
                    name: "company".to_owned(),
                    source: "path:../catalog".to_owned(),
                    git_ref: None,
                    source_kind: Some(SourceKind::Local {
                        path: "../catalog".to_owned()
                    }),
                }],
                requires: vec![
                    SelectionEntry {
                        kind: ItemKind::Skill,
                        catalog: Some("company".to_owned()),
                        pattern: "core.*".to_owned(),
                        is_rule: true,
                    },
                    SelectionEntry {
                        kind: ItemKind::Pack,
                        catalog: Some("company".to_owned()),
                        pattern: "base".to_owned(),
                        is_rule: false,
                    },
                ],
            },
        }
    );
}

#[test]
fn a_yaml_extension_is_read_like_yml() {
    let root = TempDir::new().unwrap();

    write(&root, "ambit.yaml", VALID);

    let ConfigState::Valid { file_name, .. } = snapshot(&root).config else {
        panic!("not valid");
    };

    assert_eq!(file_name, "ambit.yaml");
}

#[test]
fn an_invalid_config_names_the_line_and_the_fix() {
    let root = TempDir::new().unwrap();
    let path = write(&root, "ambit.yml", "version: 1\nextra: true\n");

    let ConfigState::Invalid {
        path: reported,
        file_name,
        problem,
    } = snapshot(&root).config
    else {
        panic!("not invalid");
    };

    assert_eq!(reported, path);
    assert_eq!(file_name, "ambit.yml");
    assert_eq!(problem.line, Some(2));
    assert!(problem.message.contains("extra"), "{problem:?}");
    assert_eq!(problem.detail.len(), 2, "{problem:?}");
}

#[test]
fn an_unsupported_version_is_invalid() {
    let root = TempDir::new().unwrap();

    write(&root, "ambit.yml", "version: 9\n");

    let ConfigState::Invalid { problem, .. } = snapshot(&root).config else {
        panic!("not invalid");
    };

    assert_eq!(problem.line, Some(1));
}

#[test]
fn both_filenames_are_ambiguous() {
    let root = TempDir::new().unwrap();

    write(&root, "ambit.yml", VALID);
    write(&root, "ambit.yaml", VALID);

    let ConfigState::Ambiguous { files, problem } = snapshot(&root).config else {
        panic!("not ambiguous");
    };

    assert_eq!(files, ["ambit.yml", "ambit.yaml"]);
    assert!(
        problem.message.contains("ambit.yml and ambit.yaml"),
        "{problem:?}"
    );
}

#[test]
fn snapshots_never_write() {
    let root = TempDir::new().unwrap();

    write(&root, "ambit.yml", "version: [\n");
    snapshot(&root);

    assert_eq!(
        fs::read(root.path().join("ambit.yml")).unwrap(),
        b"version: [\n"
    );
    assert!(!root.path().join(".ambit").exists());
}

#[test]
fn the_token_is_kept_and_cleared() {
    let engine = engine();

    assert!(!engine.has_github_token());

    engine.set_github_token(Some("gho_token".to_owned()));

    assert_eq!(engine.github_token().as_deref(), Some("gho_token"));

    engine.set_github_token(Some(String::new()));

    assert!(!engine.has_github_token());
}

#[test]
fn errors_through_the_engine_drop_its_token() {
    let engine = engine();

    engine.set_github_token(Some("plain-token-value".to_owned()));

    let error = engine.error(&ambit_core::errors::network_error(
        "cannot fetch",
        ["sent plain-token-value"],
    ));

    assert_eq!(
        error,
        EngineError::Network {
            message: "cannot fetch".to_owned(),
            detail: vec!["sent [redacted]".to_owned()],
            kind: crate::errors::NetworkKind::Other,
        }
    );
}

#[test]
fn the_engine_keeps_the_environment_it_was_given() {
    assert_eq!(
        engine().env().get("HOME").map(String::as_str),
        Some("/nonexistent")
    );
}

#[test]
fn canonical_paths_resolve_and_missing_ones_fail() {
    let root = TempDir::new().unwrap();
    let engine = engine();
    let resolved = engine
        .canonical_path(&root.path().display().to_string())
        .unwrap();

    assert_eq!(PathBuf::from(&resolved), canonicalize(root.path()).unwrap());
    assert!(matches!(
        engine.canonical_path(&root.path().join("absent").display().to_string()),
        Err(EngineError::Io { .. })
    ));
}

#[test]
fn session_state_is_per_type_and_persists() {
    #[derive(Default)]
    struct Counter(u32);

    let session = engine().open_setup("/nowhere".to_owned());

    session.with_state(|counter: &mut Counter| counter.0 += 1);
    session.with_state(|counter: &mut Counter| counter.0 += 1);
    session.with_state(|text: &mut String| text.push('x'));

    assert_eq!(session.with_state(|counter: &mut Counter| counter.0), 2);
    assert_eq!(session.with_state(|text: &mut String| text.clone()), "x");
}

#[test]
fn describes_sources_and_proposes_names() {
    let describe = |source: &str| describe_source(source.to_owned(), None).unwrap();

    assert_eq!(
        describe("acme/Skills"),
        SourceInfo {
            kind: SourceKind::Git {
                url: "https://github.com/acme/Skills.git".to_owned(),
                github: true,
                commit_ref: false,
            },
            proposed_name: "skills".to_owned(),
        }
    );
    assert_eq!(
        describe("git@gitlab.com:team/tools.git").proposed_name,
        "tools"
    );
    assert_eq!(describe("path:../shared/").proposed_name, "shared");
    assert_eq!(describe("path:.").proposed_name, "catalog");
}

#[test]
fn an_unrecognized_source_is_a_config_error() {
    let error = describe_source("not a source".to_owned(), None).unwrap_err();

    assert_eq!(
        error,
        EngineError::Config {
            message: "the source has an unrecognized source".to_owned(),
            detail: vec![
                "`not a source` matches none of the source formats ambit accepts".to_owned(),
                "use owner/repo, a git URL, `git:<url>`, or `path:./dir`".to_owned(),
            ],
            path: None,
            line: None,
        }
    );
}

#[test]
fn lists_the_five_agent_tools() {
    let tools = supported_agent_tools();
    let ids: Vec<&str> = tools.iter().map(|tool| tool.id.as_str()).collect();

    assert_eq!(ids, ["claude", "codex", "cursor", "opencode", "vscode"]);

    let opencode = &tools[3];

    assert_eq!(opencode.display_name, "OpenCode");
    assert_eq!(opencode.hooks_file, None);
    assert_eq!(opencode.limitations.len(), 1);

    let claude = &tools[0];

    assert_eq!(claude.skills_dir, ".claude/skills");
    assert_eq!(claude.personal_mcp_file, ".claude.json");
    assert_eq!(claude.limitations, Vec::<String>::new());
}
