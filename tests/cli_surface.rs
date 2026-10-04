//! The recorded help and usage-error surface, reproduced byte for byte by the built binary.
//!
//! Every case in `tests/fixtures/cli/cases.json` ends in help, the version, a usage error or a
//! flag rule, so none reaches a handler. The binary runs with a cleared environment and piped
//! stdout, so help wraps at the default 100 columns whatever terminal runs the suite.
//!
//! `UPDATE_GOLDEN=1 cargo test` rewrites the recorded output from the binary's.
#![allow(clippy::disallowed_methods)] // std::fs reads the fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use pretty_assertions::assert_eq;
use serde_json::Value;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli")
}

fn read(name: &str) -> String {
    fs::read_to_string(fixtures().join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn write(name: &str, contents: &[u8]) {
    fs::write(fixtures().join(name), contents).unwrap_or_else(|error| panic!("{name}: {error}"));
}

/// Case name to argv, in the order recorded.
fn cases() -> Vec<(String, Vec<String>)> {
    let cases: Value = serde_json::from_str(&read("cases.json")).expect("cases.json is JSON");

    cases
        .as_object()
        .expect("cases.json is an object")
        .iter()
        .map(|(name, argv)| {
            let argv = argv
                .as_array()
                .expect("argv is an array")
                .iter()
                .map(|arg| arg.as_str().expect("argv holds strings").to_owned())
                .collect();

            (name.clone(), argv)
        })
        .collect()
}

#[test]
fn reproduces_every_recorded_case() {
    let home = tempfile::tempdir().expect("a tempdir");
    let cases = cases();

    assert!(cases.len() >= 40, "only {} cases recorded", cases.len());

    for (name, argv) in cases {
        let mut command = Command::cargo_bin("ambit").expect("the ambit binary");

        command
            .args(&argv)
            .env_clear()
            .env("HOME", home.path())
            .env("XDG_CACHE_HOME", home.path().join("cache"))
            .env("AMBIT_NO_UPDATE_CHECK", "1")
            .current_dir(home.path());

        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }

        let output = command.output().expect("ambit runs");

        if std::env::var("UPDATE_GOLDEN").as_deref() == Ok("1") {
            let code = output.status.code().expect("an exit code");

            write(&format!("{name}.stdout"), &output.stdout);
            write(&format!("{name}.stderr"), &output.stderr);
            write(&format!("{name}.code"), format!("{code}\n").as_bytes());
            continue;
        }

        let code: i32 = read(&format!("{name}.code"))
            .trim()
            .parse()
            .expect("an exit code");

        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            read(&format!("{name}.stdout")),
            "{name}: stdout"
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            read(&format!("{name}.stderr")),
            "{name}: stderr"
        );
        assert_eq!(output.status.code(), Some(code), "{name}: exit code");
    }
}
