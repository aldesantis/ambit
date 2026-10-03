//! Review and apply against disposable setups: a local fixture catalog, and a bare repository for
//! the cases about revisions. Every case asserts on the files themselves.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::*;
use crate::model::state::read_state;
use crate::project::doctor::{DoctorCheck, diagnose_planned};
use crate::project::install::plan_install;
use crate::project::ownership::OWNERSHIP_CONFLICT;
use crate::project::status::item_statuses;
use crate::project::update::{UpdateOptions, check_outdated};
use crate::resolution::resolve::{BundleItem, ItemKind};
use crate::test_support::fixture_catalog::{
    FixtureGitCatalog, build_fixture_catalog, build_fixture_git_catalog,
    commit_fixture_git_revision,
};
use crate::test_support::{tempdir, test_env};
use crate::util::control::ProgressSink;
use crate::util::fs::{EntryKind, lstat_kind, mkdir_p, read_dir_names, write_text};

struct Setup {
    _temp: tempfile::TempDir,
    root: PathBuf,
    dir: PathBuf,
    env: Env,
}

/// A config on the local fixture catalog selecting `entries`, one flow mapping each.
fn local_config(entries: &[&str]) -> String {
    let requires = if entries.is_empty() {
        " []".to_owned()
    } else {
        entries.iter().fold(String::new(), |mut text, entry| {
            text.push_str("\n  - { ");
            text.push_str(entry);
            text.push_str(" }");
            text
        })
    };

    format!(
        "version: 1\n# the team's setup\ncatalogs:\n  - name: company\n    source: path:../catalog\nrequires:{requires}\n"
    )
}

impl Setup {
    fn new() -> Self {
        let temp = tempdir();
        let root = temp.path().to_path_buf();
        let dir = root.join("project");

        build_fixture_catalog(&root.join("catalog")).unwrap();
        mkdir_p(&dir).unwrap();

        Self {
            env: test_env(&root),
            _temp: temp,
            root,
            dir,
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        join(&self.dir, relative)
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.path(relative);

        mkdir_p(path.parent().unwrap()).unwrap();
        write_text(&path, text).unwrap();
    }

    fn read(&self, relative: &str) -> String {
        read_text(&self.path(relative)).unwrap()
    }

    fn exists(&self, relative: &str) -> bool {
        lstat_kind(&self.path(relative)).unwrap() != EntryKind::Missing
    }

    fn review(&self, draft: Option<&str>, file: &str) -> SetupReview {
        self.review_pinned(draft, file, IndexMap::new())
    }

    fn review_pinned(
        &self,
        draft: Option<&str>,
        file: &str,
        pins: IndexMap<String, String>,
    ) -> SetupReview {
        review_setup(
            &self.dir,
            &self.env,
            &ReviewInput {
                draft_text: draft.map(str::to_owned),
                file: file.to_owned(),
                pins,
            },
            &Control::default(),
        )
        .unwrap()
    }

    fn apply(&self, review: &SetupReview) -> Result<ApplyOutcome> {
        apply_review(&self.dir, &self.env, review, &Control::default())
    }

    /// Reviews and applies `draft` as `ambit.yml`, expecting a full install.
    fn install(&self, draft: &str) {
        let review = self.review(Some(draft), "ambit.yml");

        assert!(review.can_apply(), "{:?}", review.summary.blockers);
        assert!(matches!(
            self.apply(&review).unwrap(),
            ApplyOutcome::Installed(_)
        ));
    }

    /// Every entry under the project: file bytes, or a link's target.
    fn snapshot(&self) -> BTreeMap<String, String> {
        fn walk(dir: &Path, within: &str, found: &mut BTreeMap<String, String>) {
            for name in read_dir_names(dir).unwrap() {
                let path = dir.join(&name);
                let relative = format!("{within}{name}");

                match lstat_kind(&path).unwrap() {
                    EntryKind::Dir => walk(&path, &format!("{relative}/"), found),
                    EntryKind::Symlink => {
                        let target = std::fs::read_link(&path).unwrap();

                        found.insert(relative, format!("-> {}", target.display()));
                    }
                    _ => {
                        found.insert(relative, read_text(&path).unwrap());
                    }
                }
            }
        }

        let mut found = BTreeMap::new();

        walk(&self.dir, "", &mut found);
        found
    }
}

fn stale(result: Result<ApplyOutcome>) -> AmbitError {
    match result {
        Err(error) => error,
        Ok(outcome) => panic!("applied: {outcome:?}"),
    }
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Stage>>);

impl ProgressSink for Recorder {
    fn report(&self, progress: &Progress) {
        self.0.lock().unwrap().push(progress.stage);
    }
}

const CODE_REVIEW: &str = r#"skill: "company/code-review""#;
const SKILL_DIR: &str = ".agents/skills/code-review";

#[test]
fn reviewing_and_discarding_writes_nothing() {
    let setup = Setup::new();

    setup.install(&local_config(&[CODE_REVIEW]));

    let before = setup.snapshot();
    let review = setup.review(
        Some(&local_config(&[CODE_REVIEW, r#"pack: "company/core""#])),
        "ambit.yml",
    );

    assert!(review.can_apply());
    assert!(review.summary.config_changed);
    assert_eq!(review.summary.config.entries_added.len(), 1);
    assert!(
        review
            .summary
            .writes
            .iter()
            .any(|write| write.path == ".agents/skills/company-context")
    );
    assert!(review.summary.lock_changed);

    drop(review);

    assert_eq!(setup.snapshot(), before);
}

#[test]
fn a_canceled_review_writes_nothing() {
    let setup = Setup::new();
    let flag = Arc::new(AtomicBool::new(true));
    let control = Control::new(Some(flag), None);
    let result = review_setup(
        &setup.dir,
        &setup.env,
        &ReviewInput {
            draft_text: Some(local_config(&[CODE_REVIEW])),
            file: "ambit.yml".to_owned(),
            pins: IndexMap::new(),
        },
        &control,
    );

    assert_eq!(result.err().unwrap().code, ExitCode::Canceled);
    assert!(setup.snapshot().is_empty());
}

#[test]
fn applying_saves_the_draft_then_installs_with_progress() {
    let setup = Setup::new();
    let draft = local_config(&[CODE_REVIEW]);
    let review = setup.review(Some(&draft), "ambit.yml");
    let recorder = Arc::new(Recorder::default());
    let control = Control::new(None, Some(Arc::clone(&recorder) as Arc<dyn ProgressSink>));

    let outcome = apply_review(&setup.dir, &setup.env, &review, &control).unwrap();

    assert!(matches!(outcome, ApplyOutcome::Installed(_)));
    assert_eq!(setup.read("ambit.yml"), draft);
    assert!(setup.exists(SKILL_DIR));
    assert!(setup.exists("ambit.lock"));

    let stages = recorder.0.lock().unwrap().clone();

    for stage in [
        Stage::CheckingOwnership,
        Stage::SavingConfig,
        Stage::WritingFiles,
        Stage::WritingRecords,
    ] {
        assert!(stages.contains(&stage), "{stage:?} in {stages:?}");
    }
}

#[test]
fn an_external_config_edit_invalidates_the_review() {
    let setup = Setup::new();

    setup.install(&local_config(&[]));

    let review = setup.review(Some(&local_config(&[CODE_REVIEW])), "ambit.yml");
    let edited = format!("{}# edited elsewhere\n", local_config(&[]));

    setup.write("ambit.yml", &edited);

    let error = stale(setup.apply(&review));

    assert_eq!(error.message, STALE_REVIEW);
    assert_eq!(setup.read("ambit.yml"), edited);
    assert!(!setup.exists(SKILL_DIR));
}

#[test]
fn a_local_catalog_edit_invalidates_the_review() {
    let setup = Setup::new();
    let review = setup.review(Some(&local_config(&[CODE_REVIEW])), "ambit.yml");

    write_text(
        &setup.root.join("catalog/skills/code-review/SKILL.md"),
        "---\nname: code-review\ndescription: Changed.\n---\n",
    )
    .unwrap();

    let error = stale(setup.apply(&review));

    assert_eq!(error.message, STALE_REVIEW);
    assert!(error.detail[0].contains("\"company\""));
    assert!(!setup.exists("ambit.yml"));
}

#[test]
fn an_unmanaged_target_blocks_and_survives() {
    let setup = Setup::new();

    setup.write(&format!("{SKILL_DIR}/NOTES.md"), "mine\n");

    let review = setup.review(Some(&local_config(&[CODE_REVIEW])), "ambit.yml");
    let conflicts: Vec<&OwnershipConflict> = review
        .summary
        .blockers
        .iter()
        .filter_map(|blocker| match blocker {
            Blocker::Ownership(conflict) => Some(conflict),
            _ => None,
        })
        .collect();

    assert!(!review.can_apply());
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].path, SKILL_DIR);
    assert!(conflicts[0].detail.last().unwrap().contains("move"));
    assert!(
        !conflicts[0]
            .detail
            .iter()
            .any(|line| line.contains("adopt"))
    );
    assert!(setup.apply(&review).is_err());
    assert_eq!(setup.read(&format!("{SKILL_DIR}/NOTES.md")), "mine\n");
    assert!(!setup.exists("ambit.yml"));
}

#[test]
fn every_unmatched_entry_is_a_blocker() {
    let setup = Setup::new();
    let review = setup.review(
        Some(&local_config(&[
            CODE_REVIEW,
            r#"skill: "company/nope""#,
            r#"mcp: "company/nada""#,
        ])),
        "ambit.yml",
    );
    let unmatched = review
        .summary
        .blockers
        .iter()
        .filter(|blocker| matches!(blocker, Blocker::Unmatched { .. }))
        .count();

    assert_eq!(unmatched, 2);
    assert!(!review.can_apply());
    assert!(review.planned.is_some());
}

#[test]
fn removing_the_last_selection_uninstalls_it() {
    let setup = Setup::new();

    setup.install(&local_config(&[CODE_REVIEW]));
    setup.write("NOTES.md", "unrelated\n");

    let review = setup.review(Some(&local_config(&[])), "ambit.yml");

    assert!(review.can_apply());
    assert!(
        review
            .summary
            .removals
            .iter()
            .any(|removal| removal.path == SKILL_DIR)
    );
    assert_eq!(review.summary.diff.skills.len(), 1);
    assert!(matches!(
        setup.apply(&review).unwrap(),
        ApplyOutcome::Installed(_)
    ));
    assert!(!setup.exists(SKILL_DIR));
    assert_eq!(setup.read("NOTES.md"), "unrelated\n");
}

#[test]
fn a_yaml_config_keeps_its_name() {
    let setup = Setup::new();

    setup.write("ambit.yaml", &local_config(&[]));

    let wrong = setup.review(Some(&local_config(&[CODE_REVIEW])), "ambit.yml");

    assert!(!wrong.can_apply());

    let review = setup.review(Some(&local_config(&[CODE_REVIEW])), "ambit.yaml");

    assert!(matches!(
        setup.apply(&review).unwrap(),
        ApplyOutcome::Installed(_)
    ));
    assert_eq!(setup.read("ambit.yaml"), local_config(&[CODE_REVIEW]));
    assert!(!setup.exists("ambit.yml"));
}

#[test]
fn reapplying_the_saved_config_restores_drift() {
    let setup = Setup::new();

    setup.install(&local_config(&[CODE_REVIEW]));
    std::fs::remove_file(setup.path(SKILL_DIR)).unwrap();

    let review = setup.review(None, "ambit.yml");

    assert!(!review.summary.config_changed);
    assert_eq!(review.summary.writes.len(), 1);
    assert!(matches!(
        setup.apply(&review).unwrap(),
        ApplyOutcome::Installed(_)
    ));
    assert!(setup.exists(SKILL_DIR));
}

#[test]
fn an_installed_item_can_still_need_setup() {
    let setup = Setup::new();

    setup.install(&local_config(&[r#"skill: "company/design-tokens""#]));

    let planned = plan_install(
        &setup.dir,
        &setup.env,
        InstallOptions {
            offline: true,
            ..InstallOptions::default()
        },
        &PlanContext::default(),
    )
    .unwrap();
    let status = status_of_plan(&planned.artifacts, &planned.prior).unwrap();
    let items = item_statuses(&planned, &status).unwrap();
    let report = diagnose_planned(&setup.dir, &setup.env, &planned).unwrap();
    let skill = BundleItem {
        kind: ItemKind::Skill,
        name: "design-tokens".to_owned(),
    };

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].item, skill);
    assert_eq!(items[0].state, ArtifactState::Ok);
    assert!(report.findings.iter().any(|finding| {
        finding.check == DoctorCheck::Expects && finding.subjects.contains(&skill)
    }));
}

#[cfg(unix)]
#[test]
fn a_partial_failure_is_reported_and_retry_refuses_the_leftovers() {
    use std::os::unix::fs::PermissionsExt;

    let setup = Setup::new();
    let settings = setup.path(".claude/settings.json");

    setup.write(".claude/settings.json", "{}\n");
    std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o444)).unwrap();

    // Permissions do not stop root, so the failure this needs cannot happen there.
    if std::fs::OpenOptions::new()
        .write(true)
        .open(&settings)
        .is_ok()
    {
        return;
    }

    let draft = local_config(&[r#"pack: "company/core""#]);
    let review = setup.review(Some(&draft), "ambit.yml");

    let ApplyOutcome::NotFullyInstalled { saved, failure } = setup.apply(&review).unwrap() else {
        panic!("installed fully");
    };

    assert!(saved);
    assert_eq!(failure.stage, Stage::WritingFiles);
    assert_eq!(failure.subject, ".claude/settings.json");
    assert_eq!(setup.read("ambit.yml"), draft);
    assert!(setup.exists(".agents/skills/company-context"));
    assert_eq!(read_state(&setup.dir).unwrap().artifacts, []);

    std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o644)).unwrap();

    let error = match retry_install(
        &setup.dir,
        &setup.env,
        &review.catalog_commits,
        &Control::default(),
    ) {
        Err(error) => error,
        Ok(outcome) => panic!("retried: {outcome:?}"),
    };

    assert_eq!(error.message, OWNERSHIP_CONFLICT);
    assert!(
        error
            .detail
            .iter()
            .any(|line| line.contains(".agents/skills/company-context"))
    );
    assert!(setup.exists(".agents/skills/company-context"));
}

/// A setup whose one catalog is the git fixture's branch.
fn git_setup() -> (Setup, FixtureGitCatalog) {
    let setup = Setup::new();
    let fixture = build_fixture_git_catalog(&setup.root.join("remote")).unwrap();

    (setup, fixture)
}

fn git_config(fixture: &FixtureGitCatalog) -> String {
    format!(
        "version: 1\ncatalogs:\n  - name: company\n    source: \"{}\"\n    ref: {}\nrequires:\n  - {{ {CODE_REVIEW} }}\n",
        fixture.url, fixture.branch
    )
}

fn changed_skill(description: &str) -> String {
    format!("---\nname: code-review\ndescription: {description}\n---\n\n# Code review\n")
}

#[test]
fn a_branch_advancing_after_review_cannot_change_the_install() {
    let (setup, fixture) = git_setup();
    let review = setup.review(Some(&git_config(&fixture)), "ambit.yml");

    assert_eq!(review.catalog_commits["company"], fixture.commit);

    commit_fixture_git_revision(
        &fixture,
        &[(
            "skills/code-review/SKILL.md",
            Some(&changed_skill("Moved on.")),
        )],
        "advance",
    )
    .unwrap();

    assert!(matches!(
        setup.apply(&review).unwrap(),
        ApplyOutcome::Installed(_)
    ));
    assert!(setup.read("ambit.lock").contains(&fixture.commit));
    assert!(
        !setup
            .read(&format!("{SKILL_DIR}/SKILL.md"))
            .contains("Moved on.")
    );
}

#[test]
fn a_reviewed_update_installs_the_checked_commit_only() {
    let (setup, fixture) = git_setup();

    setup.install(&git_config(&fixture));

    let checked = commit_fixture_git_revision(
        &fixture,
        &[(
            "skills/code-review/SKILL.md",
            Some(&changed_skill("Checked.")),
        )],
        "second",
    )
    .unwrap();
    let plan = check_outdated(&setup.dir, &setup.env, &UpdateOptions::default()).unwrap();
    let latest = plan.catalogs[0].latest.clone().unwrap();

    assert_eq!(latest, checked);
    assert!(setup.read("ambit.lock").contains(&fixture.commit));

    let review = setup.review_pinned(
        None,
        "ambit.yml",
        IndexMap::from([("company".to_owned(), latest)]),
    );

    assert_eq!(review.summary.revisions.len(), 1);
    assert_eq!(review.summary.diff.skills.len(), 1);

    commit_fixture_git_revision(
        &fixture,
        &[(
            "skills/code-review/SKILL.md",
            Some(&changed_skill("Later.")),
        )],
        "third",
    )
    .unwrap();

    assert!(matches!(
        setup.apply(&review).unwrap(),
        ApplyOutcome::Installed(_)
    ));
    assert!(setup.read("ambit.lock").contains(&checked));
    assert!(
        setup
            .read(&format!("{SKILL_DIR}/SKILL.md"))
            .contains("Checked.")
    );
}
