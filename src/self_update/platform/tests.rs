use super::*;
use crate::test_support::tempdir;

#[test]
fn names_the_asset_for_every_platform_a_release_publishes() {
    assert_eq!(
        asset_name("macos", "aarch64"),
        Some("ambit-aarch64-apple-darwin.tar.xz")
    );
    assert_eq!(
        asset_name("macos", "x86_64"),
        Some("ambit-x86_64-apple-darwin.tar.xz")
    );
    assert_eq!(
        asset_name("linux", "x86_64"),
        Some("ambit-x86_64-unknown-linux-gnu.tar.xz")
    );
    assert_eq!(
        asset_name("linux", "aarch64"),
        Some("ambit-aarch64-unknown-linux-gnu.tar.xz")
    );
    assert_eq!(
        asset_name("windows", "x86_64"),
        Some("ambit-x86_64-pc-windows-msvc.zip")
    );
}

#[test]
fn names_nothing_for_a_platform_no_release_ships() {
    assert_eq!(asset_name("freebsd", "x86_64"), None);
    assert_eq!(asset_name("windows", "aarch64"), None);
}

#[test]
fn names_an_asset_for_the_machine_running_the_suite_where_one_is_published() {
    let supported = matches!(
        (std::env::consts::OS, std::env::consts::ARCH),
        ("macos" | "linux", "x86_64" | "aarch64") | ("windows", "x86_64")
    );

    assert_eq!(
        asset_name(std::env::consts::OS, std::env::consts::ARCH).is_some(),
        supported
    );
}

#[cfg(unix)]
#[test]
fn resolves_the_symlink_a_path_entry_usually_is() {
    let workspace = tempdir();
    let real = workspace.path().join("ambit-real");
    let link = workspace.path().join("ambit");

    std::fs::write(&real, "#!/bin/sh\n").expect("write the binary");
    std::os::unix::fs::symlink(&real, &link).expect("link it");

    assert_eq!(running_binary(&link), running_binary(&real));
    assert_eq!(
        running_binary(&link)
            .file_name()
            .and_then(|name| name.to_str()),
        Some("ambit-real")
    );
}

#[test]
fn falls_back_to_the_path_it_was_given_when_nothing_is_there() {
    let workspace = tempdir();
    let missing = workspace.path().join("gone");

    assert_eq!(running_binary(&missing), missing);
}

#[test]
fn accepts_a_binary_in_a_writable_directory() {
    let workspace = tempdir();
    let binary = workspace.path().join("ambit");

    std::fs::write(&binary, "").expect("write the binary");

    assert!(can_replace(&binary));
    assert_eq!(
        crate::util::fs::read_dir_names(workspace.path()).expect("list"),
        ["ambit"]
    );
}

/// Root ignores the permission bits, so the directory cannot be made unwritable for it.
#[cfg(unix)]
fn is_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).trim() == "0")
}

#[cfg(unix)]
#[test]
fn refuses_a_binary_whose_directory_cannot_be_written() {
    use std::os::unix::fs::PermissionsExt as _;

    if is_root() {
        return;
    }

    let workspace = tempdir();
    let locked = workspace.path().join("locked");

    std::fs::create_dir(&locked).expect("create the directory");
    let binary = locked.join("ambit");

    std::fs::write(&binary, "").expect("write the binary");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o500)).expect("lock it");

    let replaceable = can_replace(&binary);

    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("unlock it");
    assert!(!replaceable);
}
