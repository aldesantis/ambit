//! Replacing the binary: what stops the update before it starts, and what the swap leaves behind.
//!
//! Two properties are asserted in every failure case, because they are the ones that decide whether
//! a failed update is survivable: the old binary still has its old bytes, and nothing (no
//! `.incoming`, no `.download`) is left beside it. A self-update that half-works turns a working
//! install into no install at all.
//!
//! The Windows swap is exercised on whatever the suite runs on, by passing the flag rather than
//! reading the platform. That branch cannot be reached on a POSIX machine otherwise, and it is the
//! more delicate of the two: it moves the running binary aside before anything takes its place.

use std::path::Path;

use super::*;
use crate::errors::ExitCode;
use crate::self_update::fake_http::{Canned, FakeHttp, tar_xz, zip};
use crate::test_support::tempdir;
use crate::util::hash::sha256_hex;

const LATEST: &str = "v9.9.9";
const OLD_BYTES: &str = "the ambit that is installed\n";
const NEW_BYTES: &str = "the ambit that was released\n";
const LINUX_ASSET: &str = "ambit-x86_64-unknown-linux-gnu.tar.xz";
const WINDOWS_ASSET: &str = "ambit-x86_64-pc-windows-msvc.zip";

/// A workspace holding the installed binary.
///
/// Resolved, because the plan reports the binary with its symlinks resolved and macOS makes `/var`
/// one. Comparing against an unresolved path would fail there and pass on Linux.
struct Workspace {
    _dir: tempfile::TempDir,
    root: PathBuf,
    binary: PathBuf,
}

fn workspace() -> Workspace {
    let dir = tempdir();
    let root = running_binary(dir.path());
    let binary = root.join("ambit");

    std::fs::write(&binary, OLD_BYTES).expect("write the installed binary");

    Workspace {
        _dir: dir,
        root,
        binary,
    }
}

impl Workspace {
    fn names(&self) -> Vec<String> {
        let mut names = crate::util::fs::read_dir_names(&self.root).expect("list the workspace");
        names.sort();
        names
    }

    fn installed(&self) -> String {
        crate::util::fs::read_text(&self.binary).expect("read the binary")
    }
}

/// The archive cargo-dist publishes for the Linux target, holding `bytes` as the binary.
fn linux_archive(bytes: &str) -> Vec<u8> {
    tar_xz(&[
        ("ambit-x86_64-unknown-linux-gnu/README.md", b"readme\n"),
        ("ambit-x86_64-unknown-linux-gnu/ambit", bytes.as_bytes()),
    ])
}

/// A GitHub that serves one release: the redirect naming [`LATEST`], a `.sha256` for `archive`
/// (or for `checksum`, when given), and the archive itself.
fn release_server(asset: &'static str, archive: Vec<u8>, checksum: Option<String>) -> FakeHttp {
    let checksum = checksum.unwrap_or_else(|| sha256_hex(&archive));

    FakeHttp::new(move |url| {
        if url.ends_with("/releases/latest") {
            return Ok(Canned::redirect(&format!(
                "https://github.com/aldesantis/ambit/releases/tag/{LATEST}"
            )));
        }

        if url.ends_with(&format!("/{asset}.sha256")) {
            return Ok(Canned::ok(format!("{checksum} *{asset}\n")));
        }

        if url.ends_with(&format!("/{asset}")) {
            return Ok(Canned::ok(archive.clone()));
        }

        Ok(Canned::status(404))
    })
}

fn linux_context<'a>(binary: &Path, http: &'a FakeHttp) -> SelfContext<'a> {
    SelfContext {
        os: "linux",
        arch: "x86_64",
        exec_path: binary.to_path_buf(),
        http,
    }
}

#[test]
fn plans_a_move_to_the_latest_release() {
    let w = workspace();
    let http = release_server(LINUX_ASSET, linux_archive(NEW_BYTES), None);
    let plan = plan_self_update(&linux_context(&w.binary, &http), None).expect("a plan");

    assert_eq!(
        plan,
        SelfUpdatePlan {
            current: VERSION.to_owned(),
            target: LATEST.to_owned(),
            asset: LINUX_ASSET.to_owned(),
            binary: w.binary.clone(),
            changed: true,
        }
    );
}

#[test]
fn plans_a_named_release_without_asking_which_one_is_latest() {
    let w = workspace();
    let http = FakeHttp::unreachable();
    let plan = plan_self_update(&linux_context(&w.binary, &http), Some("0.0.1")).expect("a plan");

    assert_eq!(plan.target, "v0.0.1");
    assert!(plan.changed);
    assert!(!is_upgrade(&plan));
}

#[test]
fn reports_no_change_when_the_named_release_is_the_one_running() {
    let w = workspace();
    let http = FakeHttp::unreachable();
    let plan = plan_self_update(&linux_context(&w.binary, &http), Some(VERSION)).expect("a plan");

    assert!(!plan.changed);
}

#[test]
fn refuses_a_platform_no_release_ships_a_binary_for() {
    let w = workspace();
    let http = FakeHttp::unreachable();
    let context = SelfContext {
        os: "freebsd",
        ..linux_context(&w.binary, &http)
    };
    let error = plan_self_update(&context, None).expect_err("an unsupported platform");

    assert_eq!(error.code, ExitCode::Config);
    assert_eq!(
        error.message,
        "no ambit binary is published for freebsd-x86_64"
    );
    assert!(error.detail[1].contains("cargo install --locked --git"));
}

#[cfg(unix)]
#[test]
fn refuses_before_downloading_when_the_directory_is_read_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let w = workspace();
    let locked = w.root.join("locked");

    std::fs::create_dir(&locked).expect("create the directory");
    let installed = locked.join("ambit");

    std::fs::write(&installed, OLD_BYTES).expect("write the binary");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).expect("lock it");

    // Root ignores the permission bits, so there is nothing to refuse.
    let writable = can_replace(&installed);
    let http = FakeHttp::unreachable();
    let result = plan_self_update(&linux_context(&installed, &http), None);

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("unlock it");

    if writable {
        return;
    }

    let error = result.expect_err("a read-only directory");

    assert_eq!(error.code, ExitCode::Config);
    assert!(error.message.contains("cannot write to the directory"));
}

#[test]
fn installs_the_verified_binary_over_the_running_one() {
    let w = workspace();
    let http = release_server(LINUX_ASSET, linux_archive(NEW_BYTES), None);
    let context = linux_context(&w.binary, &http);
    let plan = plan_self_update(&context, None).expect("a plan");

    apply_self_update(&plan, &context).expect("the update applies");

    assert_eq!(w.installed(), NEW_BYTES);
    assert_eq!(w.names(), ["ambit"]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = std::fs::metadata(&w.binary)
            .expect("stat")
            .permissions()
            .mode();

        assert!(mode & 0o111 > 0);
    }

    let urls = http.urls.borrow();

    assert_eq!(
        urls[1..],
        [
            format!(
                "https://github.com/aldesantis/ambit/releases/download/{LATEST}/{LINUX_ASSET}.sha256"
            ),
            format!("https://github.com/aldesantis/ambit/releases/download/{LATEST}/{LINUX_ASSET}"),
        ]
    );
}

#[test]
fn installs_from_a_windows_zip_moving_the_old_binary_aside() {
    let w = workspace();
    // A leftover from an earlier Windows update that could not delete its displaced binary.
    std::fs::write(w.root.join("ambit.old"), OLD_BYTES).expect("write a leftover");

    let archive = zip(&[
        ("README.md", b"readme\n"),
        ("ambit.exe", NEW_BYTES.as_bytes()),
    ]);
    let http = release_server(WINDOWS_ASSET, archive, None);
    let context = SelfContext {
        os: "windows",
        ..linux_context(&w.binary, &http)
    };
    let plan = plan_self_update(&context, None).expect("a plan");

    assert_eq!(plan.asset, WINDOWS_ASSET);
    apply_self_update(&plan, &context).expect("the update applies");

    assert_eq!(w.installed(), NEW_BYTES);
    assert_eq!(w.names(), ["ambit"]);
}

#[test]
fn finds_the_binary_under_the_archives_own_directory_in_a_zip_too() {
    let w = workspace();
    let archive = zip(&[(
        "ambit-x86_64-pc-windows-msvc/ambit.exe",
        NEW_BYTES.as_bytes(),
    )]);
    let http = release_server(WINDOWS_ASSET, archive, None);
    let context = SelfContext {
        os: "windows",
        ..linux_context(&w.binary, &http)
    };
    let plan = plan_self_update(&context, None).expect("a plan");

    apply_self_update(&plan, &context).expect("the update applies");

    assert_eq!(w.installed(), NEW_BYTES);
}

#[test]
fn discards_a_download_whose_hash_does_not_match_and_keeps_the_old_binary() {
    let w = workspace();
    let http = release_server(
        LINUX_ASSET,
        linux_archive(NEW_BYTES),
        Some(sha256_hex(b"something else")),
    );
    let context = linux_context(&w.binary, &http);
    let plan = plan_self_update(&context, None).expect("a plan");
    let error = apply_self_update(&plan, &context).expect_err("a mismatch");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(
        error.message,
        format!("checksum mismatch for {LINUX_ASSET}")
    );
    assert_eq!(w.installed(), OLD_BYTES);
    assert_eq!(w.names(), ["ambit"]);
}

#[test]
fn refuses_an_archive_without_the_binary_and_keeps_the_old_one() {
    let w = workspace();
    let archive = tar_xz(&[("ambit-x86_64-unknown-linux-gnu/README.md", b"readme\n")]);
    let http = release_server(LINUX_ASSET, archive, None);
    let context = linux_context(&w.binary, &http);
    let plan = plan_self_update(&context, None).expect("a plan");
    let error = apply_self_update(&plan, &context).expect_err("no binary");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(
        error.message,
        format!("{LINUX_ASSET} does not contain the ambit binary")
    );
    assert_eq!(w.installed(), OLD_BYTES);
    assert_eq!(w.names(), ["ambit"]);
}

#[test]
fn refuses_a_verified_download_that_is_not_an_archive() {
    let w = workspace();
    let http = release_server(LINUX_ASSET, b"not an archive".to_vec(), None);
    let context = linux_context(&w.binary, &http);
    let plan = plan_self_update(&context, None).expect("a plan");
    let error = apply_self_update(&plan, &context).expect_err("an unreadable archive");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(
        error.message,
        format!("{LINUX_ASSET} does not contain the ambit binary")
    );
    assert_eq!(w.installed(), OLD_BYTES);
    assert_eq!(w.names(), ["ambit"]);
}

#[test]
fn keeps_the_old_binary_when_the_download_itself_fails() {
    let w = workspace();
    let archive = linux_archive(NEW_BYTES);
    let checksum = sha256_hex(&archive);
    let http = FakeHttp::new(move |url| {
        if url.ends_with(".sha256") {
            Ok(Canned::ok(format!("{checksum}  {LINUX_ASSET}\n")))
        } else {
            Ok(Canned::status(404))
        }
    });
    let context = linux_context(&w.binary, &http);
    let plan = plan_self_update(&context, Some(LATEST)).expect("a plan");
    let error = apply_self_update(&plan, &context).expect_err("a failed download");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(w.installed(), OLD_BYTES);
    assert_eq!(w.names(), ["ambit"]);
}

#[test]
fn refuses_a_release_whose_checksum_names_another_file() {
    let w = workspace();
    let http = FakeHttp::new(|url| {
        if url.ends_with(".sha256") {
            Ok(Canned::ok(format!("{}  other.tar.xz\n", "a".repeat(64))))
        } else {
            panic!("should not download: {url}")
        }
    });
    let context = linux_context(&w.binary, &http);
    let plan = plan_self_update(&context, Some(LATEST)).expect("a plan");
    let error = apply_self_update(&plan, &context).expect_err("no entry");

    assert_eq!(error.code, ExitCode::Network);
    assert!(error.message.contains("lists no entry"));
    assert_eq!(w.installed(), OLD_BYTES);
}

#[test]
fn renames_straight_over_the_binary_on_posix() {
    let w = workspace();
    let incoming = w.root.join("ambit.incoming");

    std::fs::write(&incoming, NEW_BYTES).expect("write the incoming binary");
    swap_in_place(&w.binary, &incoming, false).expect("swap");

    assert_eq!(w.installed(), NEW_BYTES);
    assert_eq!(w.names(), ["ambit"]);
}

#[test]
fn moves_the_running_binary_aside_first_on_windows_then_clears_it_away() {
    let w = workspace();
    let incoming = w.root.join("ambit.incoming");

    std::fs::write(&incoming, NEW_BYTES).expect("write the incoming binary");
    swap_in_place(&w.binary, &incoming, true).expect("swap");

    assert_eq!(w.installed(), NEW_BYTES);
    assert_eq!(w.names(), ["ambit"]);
}

#[test]
fn puts_the_displaced_binary_back_when_the_new_one_cannot_take_its_place() {
    let w = workspace();
    let missing = w.root.join("ambit.incoming");

    assert!(swap_in_place(&w.binary, &missing, true).is_err());
    assert_eq!(w.installed(), OLD_BYTES);
    assert_eq!(w.names(), ["ambit"]);
}
