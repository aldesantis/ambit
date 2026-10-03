//! The binary runs the git on `PATH` whatever `AMBIT_GIT_PROGRAM` says: the variable is for library
//! callers that ship their own git.
#![allow(clippy::disallowed_methods)] // std::fs writes the project; std::env reads PATH.

#[path = "../../ambit-core/tests/support/mod.rs"]
mod support;

use std::fs;

use assert_cmd::Command;

use support::fixture_catalog::build_fixture_git_catalog;

#[test]
fn ignores_ambit_git_program() {
    let root = tempfile::tempdir().expect("a tempdir");
    let fixture = build_fixture_git_catalog(&root.path().join("remote")).expect("the repository");
    let project = root.path().join("project");

    fs::create_dir(&project).expect("create the project");
    fs::write(
        project.join("ambit.yml"),
        format!(
            "version: 1\nharnesses: [claude]\ncatalogs:\n  - name: company\n    source: \"git:{}\"\nrequires:\n  - skill: company/company-context\n",
            fixture.url
        ),
    )
    .expect("write ambit.yml");

    let mut command = Command::cargo_bin("ambit").expect("the ambit binary");

    command
        .args(["install"])
        .env_clear()
        .env("HOME", root.path().join("home"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("AMBIT_NO_UPDATE_CHECK", "1")
        .env("AMBIT_GIT_PROGRAM", root.path().join("no-such-git"))
        .current_dir(&project);

    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }

    let output = command.output().expect("ambit runs");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        project
            .join(".agents/skills/company-context/SKILL.md")
            .is_file()
    );
}
