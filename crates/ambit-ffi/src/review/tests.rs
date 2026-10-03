//! Review, apply, status and health through the exported API, on a temporary setup with a local
//! catalog.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::*;
use crate::engine::EngineConfig;
use crate::status::InstallState;

const SKILL: &str = "---
name: demo
description: A demo skill.
ambit:
  expects:
    - env: DEMO_TOKEN
---

# Demo
";

struct Fixture {
    _temp: TempDir,
    root: PathBuf,
    session: Arc<SetupSession>,
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn config(selected: bool) -> String {
    let requires = if selected {
        "\n  - skill: \"company/demo\""
    } else {
        " []"
    };

    format!(
        "version: 1\ncatalogs:\n  - name: company\n    source: path:../catalog\nrequires:{requires}\n"
    )
}

impl Fixture {
    fn new() -> Self {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("setup");

        write(&temp.path().join("catalog/skills/demo/SKILL.md"), SKILL);
        fs::create_dir_all(&root).unwrap();

        let engine = Engine::new(EngineConfig {
            env: HashMap::from([
                (
                    "HOME".to_owned(),
                    temp.path().join("home").display().to_string(),
                ),
                (
                    "XDG_CACHE_HOME".to_owned(),
                    temp.path().join("cache").display().to_string(),
                ),
            ]),
        });
        let session = engine.open_setup(root.display().to_string());

        Self {
            _temp: temp,
            root,
            session,
        }
    }

    fn review(&self, draft: &str) -> Arc<Review> {
        self.session
            .review(Some(draft.to_owned()), "ambit.yml".to_owned(), None, None)
            .unwrap()
    }
}

#[test]
fn reviews_applies_and_reports_setup_required() {
    let fixture = Fixture::new();
    let review = fixture.review(&config(true));
    let summary = review.summary();

    assert!(summary.can_apply);
    assert!(summary.config_changed);
    assert_eq!(summary.capabilities.len(), 1);
    assert!(
        summary.paths.iter().any(|path| {
            path.path == ".agents/skills/demo" && path.change == PathChange::Create
        })
    );
    assert!(!fixture.root.join("ambit.yml").exists());

    let outcome = fixture.session.apply(review, None, None).unwrap();

    assert!(matches!(outcome, ApplyOutcome::Installed { .. }));
    assert_eq!(
        ambit_core::util::fs::read_text(&fixture.root.join("ambit.yml")).unwrap(),
        config(true)
    );

    let status = fixture.session.status().unwrap();

    assert_eq!(status.items.len(), 1);
    assert_eq!(status.items[0].state, InstallState::Ok);

    let health = fixture.session.health().unwrap();

    assert_eq!(health.setup_required, [status.items[0].item.clone()]);
}

#[test]
fn a_changed_config_makes_the_review_stale() {
    let fixture = Fixture::new();
    let review = fixture.review(&config(true));

    write(&fixture.root.join("ambit.yml"), &config(false));

    let error = fixture.session.apply(review, None, None).unwrap_err();

    assert!(
        matches!(error, EngineError::StaleReview { .. }),
        "{error:?}"
    );
    assert!(!fixture.root.join(".agents/skills/demo").exists());
}

#[test]
fn an_unmanaged_target_is_a_blocker_naming_the_path() {
    let fixture = Fixture::new();

    write(&fixture.root.join(".agents/skills/demo/NOTES.md"), "mine\n");

    let summary = fixture.review(&config(true)).summary();

    assert!(!summary.can_apply);
    assert!(summary.blockers.iter().any(|blocker| matches!(
        blocker,
        ReviewBlocker::Ownership { path, .. } if path == ".agents/skills/demo"
    )));
}

#[test]
fn a_canceled_review_reports_canceled() {
    let fixture = Fixture::new();
    let token = CancelToken::new();

    token.cancel();

    let error = fixture
        .session
        .review(
            Some(config(true)),
            "ambit.yml".to_owned(),
            Some(token),
            None,
        )
        .err()
        .unwrap();

    assert_eq!(error, EngineError::Canceled);
}
