//! What ambit reads off a GitHub release: which tag is latest, what a checksum line says, and the
//! bytes of one asset.
//!
//! Every case here supplies its own [`Http`], so the suite stays offline and can describe answers
//! GitHub gives rarely: a repository with no release, a truncated download, a `.sha256` naming
//! another file. Those are the paths that matter, because they are the ones that decide whether
//! unverified bytes get installed.
//!
//! The download test hashes a real file written to a real directory rather than asserting against
//! a stub, since the whole point of that function is that the hash and the file come from the same
//! stream.

use super::*;
use crate::errors::ExitCode;
use crate::self_update::fake_http::{Canned, FakeHttp};
use crate::test_support::tempdir;
use crate::util::hash::sha256_hex;

const TAG: &str = "v1.2.3";
const ASSET: &str = "ambit-x86_64-unknown-linux-gnu.tar.xz";

/// An [`Http`] that answers every URL with the same response.
fn answering(response: impl Fn() -> Canned + 'static) -> FakeHttp {
    FakeHttp::new(move |_| Ok(response()))
}

#[test]
fn is_newer_only_for_a_strictly_newer_release() {
    assert!(is_newer("0.1.0", "v0.2.0"));
    assert!(!is_newer("0.2.0", "v0.2.0"));
    assert!(!is_newer("0.3.0", "v0.2.0"));
}

#[test]
fn never_offers_a_prerelease_to_someone_on_the_release_it_leads_to() {
    assert!(!is_newer("1.0.0", "v1.0.0-rc.1"));
    assert!(is_newer("1.0.0-rc.1", "v1.0.0"));
}

#[test]
fn ignores_build_metadata_which_does_not_make_a_release_newer() {
    assert!(!is_newer("1.2.3", "v1.2.3+build.5"));
}

#[test]
fn refuses_to_guess_when_either_side_is_not_a_version() {
    assert!(!is_newer("0.1.0", "nightly"));
    assert!(!is_newer("dev", "v9.9.9"));
    assert!(!is_newer("0.1.0", "v1.2"));
}

#[test]
fn adds_the_leading_v_a_tag_has_and_a_package_version_does_not() {
    assert_eq!(as_tag("0.2.0"), "v0.2.0");
    assert_eq!(as_tag("v0.2.0"), "v0.2.0");
}

#[test]
fn builds_the_download_url_install_sh_uses() {
    assert_eq!(
        asset_url(TAG, ASSET),
        format!("https://github.com/aldesantis/ambit/releases/download/{TAG}/{ASSET}")
    );
}

#[test]
fn reads_the_tag_out_of_the_redirect() {
    let http =
        answering(|| Canned::redirect("https://github.com/aldesantis/ambit/releases/tag/v0.4.1"));

    assert_eq!(
        latest_tag(&http, METADATA_TIMEOUT).expect("a tag"),
        "v0.4.1"
    );
    assert_eq!(
        http.urls.borrow().as_slice(),
        ["https://github.com/aldesantis/ambit/releases/latest"]
    );
}

#[test]
fn decodes_a_percent_encoded_tag() {
    let http = answering(|| {
        Canned::redirect("https://github.com/aldesantis/ambit/releases/tag/v1.0.0%2Bbuild")
    });

    assert_eq!(
        latest_tag(&http, METADATA_TIMEOUT).expect("a tag"),
        "v1.0.0+build"
    );
}

#[test]
fn refuses_when_the_redirect_names_no_tag_which_is_a_repository_with_no_release() {
    let http = answering(|| Canned::redirect("https://github.com/aldesantis/ambit/releases"));
    let error = latest_tag(&http, METADATA_TIMEOUT).expect_err("no tag");

    assert_eq!(error.code, ExitCode::Network);
    assert!(
        error
            .message
            .contains("did not name a latest ambit release")
    );
    assert_eq!(
        error.detail[0],
        "https://github.com/aldesantis/ambit/releases/latest answered 302 pointing at \"https://github.com/aldesantis/ambit/releases\""
    );
}

#[test]
fn refuses_when_the_request_itself_fails() {
    let http = FakeHttp::new(|_| Err("getaddrinfo ENOTFOUND github.com".to_owned()));
    let error = latest_tag(&http, METADATA_TIMEOUT).expect_err("a transport failure");

    assert_eq!(error.code, ExitCode::Network);
    assert!(error.detail.join(" ").contains("ENOTFOUND"));
}

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn finds_the_line_for_one_asset() {
    let file = format!(
        "{}  ambit-aarch64-apple-darwin.tar.xz\n{HASH}  {ASSET}",
        "b".repeat(64)
    );

    assert_eq!(checksum_for(&file, ASSET).expect("a hash"), HASH);
}

#[test]
fn reads_the_binary_mode_star_sha256sum_writes() {
    assert_eq!(
        checksum_for(&format!("{HASH} *{ASSET}\n"), ASSET).expect("a hash"),
        HASH
    );
}

#[test]
fn reads_a_file_holding_the_hash_alone() {
    assert_eq!(
        checksum_for(&format!("{HASH}\n"), ASSET).expect("a hash"),
        HASH
    );
}

#[test]
fn does_not_match_an_asset_whose_name_merely_ends_with_the_one_asked_for() {
    let error = checksum_for(&format!("{HASH}  extra-{ASSET}\n"), ASSET).expect_err("no entry");

    assert_eq!(error.code, ExitCode::Network);
}

#[test]
fn refuses_when_the_file_lists_no_line_for_the_asset() {
    let error = checksum_for(
        &format!("{HASH}  ambit-aarch64-apple-darwin.tar.xz\n"),
        ASSET,
    )
    .expect_err("no entry");

    assert_eq!(error.code, ExitCode::Network);
    assert_eq!(
        error.message,
        format!("{ASSET}.sha256 lists no entry for {ASSET}")
    );
}

#[test]
fn returns_the_body() {
    let http = answering(|| Canned::ok("hello\n"));

    assert_eq!(
        fetch_asset_text(&http, TAG, "a.sha256", METADATA_TIMEOUT).expect("the body"),
        "hello\n"
    );
}

#[test]
fn refuses_a_release_that_does_not_attach_the_asset() {
    let http = answering(|| Canned::status(404));
    let error = fetch_asset_text(&http, TAG, "a.sha256", METADATA_TIMEOUT).expect_err("a 404");

    assert_eq!(error.code, ExitCode::Network);
    assert!(error.detail.join(" ").contains("404"));
}

#[test]
fn writes_the_bytes_and_returns_the_hash_of_the_same_stream() {
    let workspace = tempdir();
    let bytes: Vec<u8> = (0..200_000_u32)
        .map(|index| u8::try_from(index % 256).expect("a byte"))
        .collect();
    let expected = sha256_hex(&bytes);
    let destination = workspace.path().join(ASSET);
    let served = bytes.clone();
    let http = answering(move || Canned::ok(served.clone()));

    assert_eq!(
        download_asset(&http, TAG, ASSET, &destination, DOWNLOAD_TIMEOUT).expect("a download"),
        expected
    );
    assert_eq!(std::fs::read(&destination).expect("read it back"), bytes);
}

#[cfg(unix)]
#[test]
fn writes_it_executable_since_it_replaces_something_that_has_to_run() {
    use std::os::unix::fs::PermissionsExt as _;

    let workspace = tempdir();
    let destination = workspace.path().join(ASSET);

    download_asset(
        &answering(|| Canned::ok("#!/bin/sh\n")),
        TAG,
        ASSET,
        &destination,
        DOWNLOAD_TIMEOUT,
    )
    .expect("a download");

    let mode = std::fs::metadata(&destination)
        .expect("stat")
        .permissions()
        .mode();

    assert!(mode & 0o111 > 0);
}

#[test]
fn refuses_a_response_that_is_not_a_download() {
    let workspace = tempdir();
    let destination = workspace.path().join(ASSET);
    let error = download_asset(
        &answering(|| Canned::status(404)),
        TAG,
        ASSET,
        &destination,
        DOWNLOAD_TIMEOUT,
    )
    .expect_err("a 404");

    assert_eq!(error.code, ExitCode::Network);
    assert!(error.detail.join(" ").contains("404"));
    assert!(!destination.exists());
}
