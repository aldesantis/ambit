use std::path::{Path, PathBuf};

use super::*;
use crate::errors::ExitCode;
use crate::model::git::{CACHE_DIRNAME, cache_root, git_cache_key};
use crate::util::path::{join, resolve};

const SUBJECT: &str = "catalog \"company\"";
const WHERE: &str = "(ambit.yml)";

fn request(source: &str, r#ref: Option<&str>) -> SourceRequest {
    SourceRequest {
        source: source.to_owned(),
        r#ref: r#ref.map(str::to_owned),
        subject: SUBJECT.to_owned(),
        r#where: WHERE.to_owned(),
        ..SourceRequest::default()
    }
}

fn parse(source: &str, r#ref: Option<&str>) -> Source {
    parse_source(&request(source, r#ref)).expect("the source parses")
}

fn rejection(source: &str, r#ref: Option<&str>) -> AmbitError {
    let error = parse_source(&request(source, r#ref))
        .expect_err(&format!("expected `{source}` to be rejected"));

    assert_eq!(error.code, ExitCode::Config, "{}", error.format());
    error
}

fn git(url: &str, r#ref: Option<&str>) -> Source {
    Source::Git {
        url: url.to_owned(),
        r#ref: r#ref.map(str::to_owned),
    }
}

#[test]
fn reads_a_bare_owner_repo_as_a_github_repository() {
    assert_eq!(
        parse("acme/skills", None),
        git("https://github.com/acme/skills.git", None)
    );
}

#[test]
fn takes_the_ref_from_an_owner_repo_at_ref_shorthand() {
    assert_eq!(
        parse("acme/skills@v1.2.0", None),
        git("https://github.com/acme/skills.git", Some("v1.2.0"))
    );
}

#[test]
fn accepts_a_shorthand_ref_that_agrees_with_the_entrys_own() {
    assert_eq!(
        parse("acme/skills@main", Some("main")),
        git("https://github.com/acme/skills.git", Some("main"))
    );
}

#[test]
fn refuses_a_shorthand_ref_that_contradicts_the_entrys_own() {
    let error = rejection("acme/skills@v1", Some("v2"));

    assert_eq!(error.message, format!("{SUBJECT} names two refs {WHERE}"));
    assert!(
        error
            .detail
            .join("\n")
            .contains("`source` ends with \"@v1\" and `ref` says \"v2\"")
    );
}

#[test]
fn takes_an_https_url_exactly_as_written() {
    assert_eq!(
        parse("https://github.com/acme/skills", Some("main")),
        git("https://github.com/acme/skills", Some("main"))
    );
}

#[test]
fn takes_an_ssh_remote_exactly_as_written() {
    assert_eq!(
        parse("git@github.com:acme/skills.git", None),
        git("git@github.com:acme/skills.git", None)
    );
}

#[test]
fn hands_git_whatever_follows_git_prefix() {
    assert_eq!(
        parse("git:ssh://build@git.acme.test:2222/srv/skills.git", None),
        git("ssh://build@git.acme.test:2222/srv/skills.git", None)
    );
}

#[test]
fn reads_path_as_a_directory_prefix_stripped() {
    assert_eq!(
        parse("path:../catalog", None),
        Source::Path {
            directory: "../catalog".to_owned()
        }
    );
}

#[test]
fn refuses_a_source_in_no_recognized_format() {
    let error = rejection("../catalog", None);

    assert_eq!(
        error.message,
        format!("{SUBJECT} has an unrecognized source {WHERE}")
    );
    assert!(
        error
            .detail
            .join("\n")
            .contains("use owner/repo, a git URL")
    );
}

#[test]
fn refuses_a_path_naming_no_directory() {
    assert_eq!(
        rejection("path:", None).message,
        format!("{SUBJECT} has an empty path source {WHERE}")
    );
}

#[test]
fn refuses_a_git_naming_no_repository() {
    assert_eq!(
        rejection("git:", None).message,
        format!("{SUBJECT} has an empty git source {WHERE}")
    );
}

#[test]
fn keys_a_repository_by_host_owner_and_name() {
    assert_eq!(
        git_cache_key("https://github.com/acme/skills.git"),
        "github.com/acme/skills"
    );
}

#[test]
fn keys_the_two_spellings_of_one_repository_the_same_way() {
    assert_eq!(
        git_cache_key("https://github.com/acme/skills"),
        git_cache_key("https://github.com/acme/skills.git")
    );
}

#[test]
fn keys_an_ssh_remote_by_the_host_it_names_not_by_the_user() {
    assert_eq!(
        git_cache_key("git@git.acme.test:acme/skills.git"),
        "git.acme.test/acme/skills"
    );
}

#[test]
fn keys_a_url_with_a_port_and_a_deep_path_by_every_segment() {
    assert_eq!(
        git_cache_key("ssh://build@git.acme.test:2222/team/acme/skills.git"),
        "git.acme.test/team/acme/skills"
    );
}

#[test]
fn keys_a_local_repository_under_local() {
    assert_eq!(
        git_cache_key("file:///srv/git/skills.git"),
        "local/srv/git/skills"
    );
}

#[test]
fn keeps_a_key_inside_the_cache_whatever_a_url_puts_in_a_segment() {
    let key = git_cache_key("https://git.acme.test/../../etc/skills.git");

    assert!(!key.split('/').any(|segment| segment == ".."));
    assert!(resolve(Path::new("/cache"), &key).starts_with("/cache/"));
    assert_ne!(resolve(Path::new("/cache"), &key), PathBuf::from("/cache"));
}

fn env(pairs: &[(&str, &str)]) -> Env {
    pairs
        .iter()
        .map(|&(name, value)| (name.to_owned(), value.to_owned()))
        .collect()
}

#[test]
fn sits_under_xdg_cache_home_when_the_environment_names_one() {
    assert_eq!(
        cache_root(&env(&[
            ("XDG_CACHE_HOME", "/tmp/xdg"),
            ("HOME", "/home/jane")
        ])),
        join(Path::new("/tmp/xdg"), CACHE_DIRNAME)
    );
}

#[test]
fn falls_back_to_dot_cache() {
    assert_eq!(
        cache_root(&env(&[("HOME", "/home/jane")])),
        join(Path::new("/home/jane"), ".cache/ambit")
    );
}

#[test]
fn ignores_an_xdg_cache_home_set_to_nothing() {
    assert_eq!(
        cache_root(&env(&[("XDG_CACHE_HOME", ""), ("HOME", "/home/jane")])),
        join(Path::new("/home/jane"), ".cache/ambit")
    );
}

#[test]
fn resolves_a_path_source_against_the_project() {
    let root = crate::test_support::tempdir();
    std::fs::create_dir(root.path().join("catalog")).expect("create the catalog");
    let project = root.path().join("project");
    let context = SourceContext {
        project_dir: project.clone(),
        ..SourceContext::default()
    };

    let resolved =
        resolve_source(&request("path:../catalog", None), &context).expect("the source resolves");

    assert_eq!(resolved.root, root.path().join("catalog"));
    assert_eq!(resolved.commit, None);

    let error = resolve_source(&request("path:./missing", None), &context)
        .expect_err("a missing directory is refused");

    assert_eq!(error.code, ExitCode::Config);
    assert_eq!(
        error.message,
        format!("{SUBJECT} is not a directory {WHERE}")
    );
}
