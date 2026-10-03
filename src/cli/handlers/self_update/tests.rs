//! The handler's report: the plan as text and as JSON, and what `--dry-run` leaves alone.
//!
//! Driven through [`run_self_update`] with a described machine and a fake GitHub, since the
//! shipped handler reads the real executable and the real network.

use std::path::{Path, PathBuf};

use super::*;
use crate::cli::CaptureIo;
use crate::cli::commands::{CommandOptions, OptionValue};
use crate::self_update::fake_http::{Canned, FakeHttp, tar_xz};
use crate::self_update::platform::running_binary;
use crate::test_support::tempdir;
use crate::util::env::Env;
use crate::util::hash::sha256_hex;
use crate::version::VERSION;

const ASSET: &str = "ambit-x86_64-unknown-linux-gnu.tar.xz";

fn release(tag: &'static str) -> FakeHttp {
    let archive = tar_xz(&[("ambit-x86_64-unknown-linux-gnu/ambit", b"new\n")]);
    let checksum = sha256_hex(&archive);

    FakeHttp::new(move |url| {
        if url.ends_with("/releases/latest") {
            Ok(Canned::redirect(&format!(
                "https://github.com/aldesantis/ambit/releases/tag/{tag}"
            )))
        } else if url.ends_with(".sha256") {
            Ok(Canned::ok(format!("{checksum}  {ASSET}\n")))
        } else {
            Ok(Canned::ok(archive.clone()))
        }
    })
}

fn run(
    binary: &Path,
    http: &FakeHttp,
    args: &[&str],
    flags: &[&str],
) -> (Result<ExitCode>, CaptureIo) {
    let env = Env::new();
    let mut io = CaptureIo::default();
    let mut options = CommandOptions::default();

    for flag in flags {
        options.0.insert((*flag).to_owned(), OptionValue::Flag);
    }

    let mut ctx = CommandContext {
        options,
        args: args.iter().map(|&arg| arg.to_owned()).collect(),
        cwd: PathBuf::from("/"),
        env: &env,
        io: &mut io,
    };
    let context = SelfContext {
        os: "linux",
        arch: "x86_64",
        exec_path: binary.to_path_buf(),
        http,
    };
    let result = run_self_update(&mut ctx, &context);

    (result, io)
}

fn installed_binary() -> (tempfile::TempDir, PathBuf) {
    let dir = tempdir();
    let binary = running_binary(dir.path()).join("ambit");

    std::fs::write(&binary, "old\n").expect("write the binary");
    (dir, binary)
}

#[test]
fn prints_the_plan_and_installs_the_latest_release() {
    let (_dir, binary) = installed_binary();
    let (result, io) = run(&binary, &release("v99.0.0"), &[], &[]);

    assert_eq!(result.expect("success"), ExitCode::Success);
    assert_eq!(
        io.out,
        [
            format!("current  {VERSION}"),
            "target   v99.0.0".to_owned(),
            format!("asset    {ASSET}"),
            format!("binary   {}", binary.display()),
            String::new(),
            "installed ambit v99.0.0".to_owned(),
        ]
    );
    assert_eq!(crate::util::fs::read_text(&binary).expect("read"), "new\n");
}

#[test]
fn reports_a_downgrade_as_one() {
    let (_dir, binary) = installed_binary();
    let (result, io) = run(&binary, &release("v99.0.0"), &["0.0.1"], &[]);

    result.expect("success");
    assert_eq!(
        io.out.last().map(String::as_str),
        Some(format!("installed ambit v0.0.1, a downgrade from {VERSION}").as_str())
    );
}

#[test]
fn changes_nothing_on_a_dry_run() {
    let (_dir, binary) = installed_binary();
    let http = release("v99.0.0");
    let (result, io) = run(&binary, &http, &[], &["dryRun"]);

    result.expect("success");
    assert_eq!(
        io.out.last().map(String::as_str),
        Some("would install ambit v99.0.0")
    );
    assert_eq!(crate::util::fs::read_text(&binary).expect("read"), "old\n");
    assert_eq!(http.calls.get(), 1);
}

#[test]
fn says_so_when_the_release_is_already_installed() {
    let (_dir, binary) = installed_binary();
    let http = FakeHttp::unreachable();
    let (result, io) = run(&binary, &http, &[VERSION], &[]);

    result.expect("success");
    assert_eq!(
        io.out.last().map(String::as_str),
        Some(format!("ambit v{VERSION} is already installed").as_str())
    );
}

#[test]
fn reports_the_plan_as_json() {
    let (_dir, binary) = installed_binary();
    let (result, io) = run(&binary, &release("v99.0.0"), &[], &["json", "dryRun"]);

    result.expect("success");
    assert_eq!(
        io.out,
        [format!(
            "{{\n  \"asset\": \"{ASSET}\",\n  \"binary\": {},\n  \"changed\": true,\n  \"current\": \"{VERSION}\",\n  \"installed\": false,\n  \"target\": \"v99.0.0\",\n  \"upgrade\": true\n}}",
            crate::util::json::stringify(&binary.to_string_lossy().into_owned().into())
        )]
    );
}

#[test]
fn passes_a_refusal_through_without_printing() {
    let (_dir, binary) = installed_binary();
    let http = FakeHttp::new(|_| Err("offline".to_owned()));
    let (result, io) = run(&binary, &http, &[], &[]);
    let error = result.expect_err("no network");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(io.out, Vec::<String>::new());
}
