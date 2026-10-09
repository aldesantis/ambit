#![allow(clippy::disallowed_methods)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use pretty_assertions::assert_eq;

use support::fixture_catalog::build_fixture_catalog;

const CATALOG_NAME: &str = "company";

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/resolve")
}

fn entry(kind: &str, address: &str) -> String {
    format!("  - {{ {kind}: \"{CATALOG_NAME}/{address}\" }}")
}

fn profiles() -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("empty", vec![]),
        ("core", vec![entry("pack", "core")]),
        (
            "engineering",
            vec![
                entry("pack", "function.engineering"),
                entry("pack", "function.engineering.*"),
            ],
        ),
        (
            "core-and-engineering",
            vec![
                entry("pack", "core"),
                entry("pack", "function.engineering"),
                entry("pack", "function.engineering.*"),
            ],
        ),
        (
            "frontend",
            vec![entry("pack", "function.engineering.frontend")],
        ),
        ("project", vec![entry("pack", "project.acme")]),
    ]
}

fn write_profile(project_dir: &Path, requires: &[String]) {
    let list = if requires.is_empty() {
        "[]".to_owned()
    } else {
        format!("\n{}", requires.join("\n"))
    };

    fs::write(
        project_dir.join("ambit.yml"),
        format!(
            "version: 1\ncatalogs:\n  - name: {CATALOG_NAME}\n    source: path:../catalog\nrequires: {list}\n"
        ),
    )
    .expect("write ambit.yml");
}

fn expect_golden(name: &str, actual: &str) {
    let file = golden_dir().join(format!("{name}.json"));

    if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
        fs::create_dir_all(golden_dir()).expect("create the golden directory");
        fs::write(&file, actual).expect("write the golden file");

        return;
    }

    let Ok(expected) = fs::read_to_string(&file) else {
        panic!(
            "missing golden file {}; regenerate with UPDATE_GOLDEN=1 cargo test",
            file.display()
        );
    };

    assert_eq!(
        actual, expected,
        "golden mismatch for {name}; UPDATE_GOLDEN=1 cargo test to accept"
    );
}

#[test]
fn matches_the_golden_bundle_for_every_profile() {
    for (name, requires) in profiles() {
        let root = tempfile::tempdir().expect("a tempdir");
        let project_dir = root.path().join("project");

        build_fixture_catalog(&root.path().join("catalog")).expect("build the fixture catalog");
        fs::create_dir_all(&project_dir).expect("create the project");
        write_profile(&project_dir, &requires);

        let mut command = Command::cargo_bin("ambit").expect("the ambit binary");

        command
            .args(["resolve", "--json", "--project"])
            .arg(&project_dir)
            .env_clear()
            .env("HOME", root.path().join("home"))
            .env("XDG_CACHE_HOME", root.path().join("cache"))
            .env("AMBIT_NO_UPDATE_CHECK", "1")
            .current_dir(root.path());

        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }

        let output = command.output().expect("ambit runs");

        assert_eq!(
            output.status.code(),
            Some(0),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        expect_golden(name, &String::from_utf8_lossy(&output.stdout));
    }
}
