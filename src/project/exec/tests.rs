//! The `exec` digest recipe, field by field on hand-built items.

use std::path::PathBuf;

use pretty_assertions::assert_eq;

use super::*;
use crate::model::hook_entity::HookEvent;
use crate::model::mcp_entity::{HttpTransport, StdioTransport};

const CATALOG_NAME: &str = "company";

// The recipe

fn hook(r#type: HookType, command: &str) -> MergedHook {
    MergedHook {
        name: "guard".to_owned(),
        description: None,
        event: HookEvent::PreToolUse,
        matcher: Some("Bash".to_owned()),
        r#type,
        command: command.to_owned(),
        timeout: None,
        expects: Vec::new(),
        catalog: CATALOG_NAME.to_owned(),
        path: "hooks/guard".to_owned(),
        commit: None,
        catalog_root: PathBuf::new(),
    }
}

fn stdio(command: &str, args: &[&str], env: &[(&str, &str)]) -> McpTransport {
    McpTransport::Stdio(StdioTransport {
        command: command.to_owned(),
        args: args.iter().map(|&arg| arg.to_owned()).collect(),
        env: env
            .iter()
            .map(|&(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
    })
}

fn http(url: &str, bearer: Option<&str>, headers: &[(&str, &str)]) -> McpTransport {
    McpTransport::Http(HttpTransport {
        url: url.to_owned(),
        bearer_token_env_var: bearer.map(str::to_owned),
        headers: headers
            .iter()
            .map(|&(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
    })
}

#[test]
fn a_hook_digest_is_stable_and_moves_with_every_field_that_decides_what_runs() {
    let base = hook(HookType::Command, "echo hi");
    let digest = hook_exec(&base, None);

    assert_eq!(hook_exec(&base.clone(), None), digest);
    assert!(digest.starts_with("sha256-"));

    let mut event = base.clone();
    event.event = HookEvent::PostToolUse;

    let mut matcher = base.clone();
    matcher.matcher = None;

    let mut empty_matcher = base.clone();
    empty_matcher.matcher = Some(String::new());

    let mut command = base.clone();
    command.command = "echo bye".to_owned();

    let mut r#type = base.clone();
    r#type.r#type = HookType::Script;

    for (field, changed) in [
        ("event", &event),
        ("matcher", &matcher),
        ("type", &r#type),
        ("command", &command),
    ] {
        assert_ne!(hook_exec(changed, None), digest, "{field}");
    }

    assert_ne!(hook_exec(&matcher, None), hook_exec(&empty_matcher, None));

    // Nothing else changes what runs.
    let mut cosmetic = base.clone();
    cosmetic.timeout = Some(30);
    cosmetic.description = Some("says hi".to_owned());
    cosmetic.catalog = "elsewhere".to_owned();

    assert_eq!(hook_exec(&cosmetic, None), digest);
}

#[test]
fn a_script_hook_digest_covers_the_scripts_tree_and_a_command_hook_ignores_one() {
    let script = hook(HookType::Script, "guard.sh");
    let one = hook_exec(&script, Some("sha256-one"));

    assert_ne!(hook_exec(&script, Some("sha256-two")), one);
    assert_ne!(hook_exec(&script, None), one);

    let command = hook(HookType::Command, "guard.sh");

    assert_eq!(
        hook_exec(&command, Some("sha256-one")),
        hook_exec(&command, None)
    );
}

#[test]
fn a_stdio_digest_moves_with_the_command_each_argument_and_the_environment() {
    let digest = mcp_exec(&stdio("npx", &["-y", "pkg"], &[("A", "1")]));

    for (field, changed) in [
        ("command", stdio("node", &["-y", "pkg"], &[("A", "1")])),
        ("an argument", stdio("npx", &["-y", "other"], &[("A", "1")])),
        (
            "argument order",
            stdio("npx", &["pkg", "-y"], &[("A", "1")]),
        ),
        (
            "an extra argument",
            stdio("npx", &["-y", "pkg", "x"], &[("A", "1")]),
        ),
        ("an env name", stdio("npx", &["-y", "pkg"], &[("B", "1")])),
        ("an env value", stdio("npx", &["-y", "pkg"], &[("A", "2")])),
        ("no env", stdio("npx", &["-y", "pkg"], &[])),
    ] {
        assert_ne!(mcp_exec(&changed), digest, "{field}");
    }

    // An argument cannot pass for an env entry, nor two arguments for one.
    assert_ne!(
        mcp_exec(&stdio("npx", &["A", "1"], &[])),
        mcp_exec(&stdio("npx", &[], &[("A", "1")]))
    );
    assert_ne!(
        mcp_exec(&stdio("npx", &["a b"], &[])),
        mcp_exec(&stdio("npx", &["a", "b"], &[]))
    );

    // The order a catalog wrote its variables in runs the same process.
    assert_eq!(
        mcp_exec(&stdio("npx", &[], &[("A", "1"), ("B", "2")])),
        mcp_exec(&stdio("npx", &[], &[("B", "2"), ("A", "1")]))
    );
}

#[test]
fn an_http_digest_moves_with_the_url_the_token_variable_and_each_header() {
    let digest = mcp_exec(&http("https://a", Some("TOKEN"), &[("X-Key", "${KEY}")]));

    for (field, changed) in [
        (
            "url",
            http("https://b", Some("TOKEN"), &[("X-Key", "${KEY}")]),
        ),
        (
            "bearer",
            http("https://a", Some("OTHER"), &[("X-Key", "${KEY}")]),
        ),
        ("no bearer", http("https://a", None, &[("X-Key", "${KEY}")])),
        (
            "header name",
            http("https://a", Some("TOKEN"), &[("X-Other", "${KEY}")]),
        ),
        (
            "header value",
            http("https://a", Some("TOKEN"), &[("X-Key", "${OTHER}")]),
        ),
    ] {
        assert_ne!(mcp_exec(&changed), digest, "{field}");
    }

    assert_ne!(
        mcp_exec(&http("npx", None, &[])),
        mcp_exec(&stdio("npx", &[], &[]))
    );
}
