use super::*;
use crate::errors::{AmbitError, ExitCode};
use crate::model::expectation::ExpectationKind;
use crate::model::reference::Reference;
use crate::model::yaml::parse_yaml_mapping;

const FILE: &str = "hooks/block-rm/hook.yml";

fn try_parse(text: &str) -> Result<HookEntity> {
    parse_hook_entity(&parse_yaml_mapping(text, FILE).unwrap())
}

fn parse(text: &str) -> HookEntity {
    try_parse(text).unwrap()
}

fn rejection(text: &str) -> AmbitError {
    let error = try_parse(text).expect_err("expected the hook to be rejected");

    assert_eq!(error.code, ExitCode::Config, "{}", error.format());
    error
}

#[test]
fn reads_every_field() {
    assert_eq!(
        parse(
            &[
                "name: block-rm",
                "description: Refuses a destructive rm before it runs",
                "event: PreToolUse",
                "matcher: Bash",
                "type: script",
                "command: hook.sh",
                "timeout: 30",
                "expects: [{ env: SOME_TOKEN }]",
                "",
            ]
            .join("\n")
        ),
        HookEntity {
            name: "block-rm".to_owned(),
            description: Some("Refuses a destructive rm before it runs".to_owned()),
            event: HookEvent::PreToolUse,
            matcher: Some("Bash".to_owned()),
            r#type: HookType::Script,
            command: "hook.sh".to_owned(),
            timeout: Some(30),
            expects: vec![Reference {
                kind: ExpectationKind::Env,
                name: "SOME_TOKEN".to_owned(),
            }],
        }
    );
}

#[test]
fn defaults_every_optional_field_and_omits_the_ones_that_have_no_default() {
    assert_eq!(
        parse("name: greet\nevent: SessionStart\ntype: command\ncommand: echo hi\n"),
        HookEntity {
            name: "greet".to_owned(),
            description: None,
            event: HookEvent::SessionStart,
            matcher: None,
            r#type: HookType::Command,
            command: "echo hi".to_owned(),
            timeout: None,
            expects: vec![],
        }
    );
}

#[test]
fn accepts_every_event_it_publishes() {
    for &event in HOOK_EVENTS {
        assert_eq!(
            parse(&format!(
                "name: h\nevent: {event}\ntype: command\ncommand: run\n"
            ))
            .event,
            event
        );
    }
}

#[test]
fn accepts_a_matcher_on_every_matchable_event() {
    for &event in MATCHABLE_EVENTS {
        assert_eq!(
            parse(&format!(
                "name: h\nevent: {event}\nmatcher: Bash\ntype: command\ncommand: run\n"
            ))
            .matcher
            .as_deref(),
            Some("Bash")
        );
    }
}

#[test]
fn takes_a_command_types_command_as_an_opaque_string() {
    assert_eq!(
        parse("name: h\nevent: Stop\ntype: command\ncommand: ${AMBIT_BIN} --write\n").command,
        "${AMBIT_BIN} --write"
    );
}

#[test]
fn accepts_a_script_whose_name_reads_like_a_bare_program() {
    assert_eq!(
        parse("name: h\nevent: Stop\ntype: script\ncommand: guard\n").command,
        "guard"
    );
}

#[test]
fn accepts_a_script_with_the_script_first_and_the_rest_arguments() {
    assert_eq!(
        parse("name: h\nevent: Stop\ntype: script\ncommand: hook.js --strict\n").command,
        "hook.js --strict"
    );
}

#[test]
fn rejects_an_unknown_event_naming_the_supported_set() {
    let error = rejection("name: h\nevent: PostToolUsage\ntype: command\ncommand: run\n");

    assert_eq!(
        error.format(),
        [
            format!("error: unknown hook event \"PostToolUsage\" ({FILE} line 2)"),
            "       supported events: SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, Stop, SubagentStop, PreCompact, SessionEnd".to_owned(),
            "       replace `PostToolUsage` with one of them".to_owned(),
        ]
        .join("\n")
    );
}

#[test]
fn rejects_a_matcher_on_an_event_that_carries_no_tool_name() {
    let error =
        rejection("name: h\nevent: SessionStart\nmatcher: Bash\ntype: command\ncommand: run\n");

    assert!(error.format().contains(&format!(
        "error: `matcher` is not meaningful for SessionStart ({FILE} line 3)"
    )));
    assert!(
        error
            .format()
            .contains("it filters on a tool name, so it applies to: PreToolUse, PostToolUse")
    );
}

#[test]
fn requires_a_name_an_event_a_command_and_a_type() {
    for (text, key) in [
        ("event: Stop\ntype: command\ncommand: run\n", "name"),
        ("name: h\ntype: command\ncommand: run\n", "event"),
        ("name: h\nevent: Stop\ntype: command\n", "command"),
        ("name: h\nevent: Stop\ncommand: guard.sh\n", "type"),
    ] {
        assert!(
            rejection(text)
                .format()
                .contains(&format!("missing required key \"{key}\""))
        );
    }
}

#[test]
fn rejects_an_unknown_type_naming_both() {
    let error = rejection("name: h\nevent: Stop\ntype: shell\ncommand: run\n");

    assert_eq!(
        error.format(),
        [
            format!("error: unknown hook type \"shell\" ({FILE} line 3)"),
            "       `command` runs a command line as written; `script` runs a file the hook's own directory ships".to_owned(),
            "       replace `shell` with one of them".to_owned(),
        ]
        .join("\n")
    );
}

#[test]
fn requires_a_whole_number_timeout() {
    assert!(
        rejection("name: h\nevent: Stop\ntype: command\ncommand: run\ntimeout: 1.5\n")
            .format()
            .contains("\"timeout\" must be an integer")
    );
}

#[test]
fn rejects_an_unknown_key() {
    let error = rejection("name: h\nevent: Stop\ntype: command\ncommand: run\nmatchers: Bash\n");

    assert!(error.format().contains("unknown key \"matchers\""));
    assert!(error.format().contains(
        "accepted keys: command, description, event, expects, matcher, name, timeout, type"
    ));
}

mod a_script_whose_command_cannot_be_a_path_inside_the_hook {
    use super::*;

    #[test]
    fn rejects_an_absolute_path() {
        let error = rejection("name: h\nevent: Stop\ntype: script\ncommand: /usr/bin/guard\n");

        assert!(
            error.format().contains(
                "`type: script` needs a path inside the hook, and it is an absolute path"
            )
        );
        assert!(
            error
                .format()
                .contains("to run something else, say `type: command` instead")
        );
    }

    #[test]
    fn rejects_one_climbing_out_through_dot_dot() {
        assert!(
            rejection("name: h\nevent: Stop\ntype: script\ncommand: ../shared/guard.sh\n")
                .format()
                .contains("it climbs out through `..`")
        );
    }

    #[test]
    fn accepts_dot_slash_and_a_nested_path_which_are_both_inside_it() {
        assert_eq!(
            parse("name: h\nevent: Stop\ntype: script\ncommand: ./guard.sh\n").command,
            "./guard.sh"
        );
        assert_eq!(
            parse("name: h\nevent: Stop\ntype: script\ncommand: bin/guard.sh\n").command,
            "bin/guard.sh"
        );
    }
}

#[test]
fn reads_the_program_and_the_script_reference() {
    assert_eq!(command_program("  guard.sh --strict "), "guard.sh");
    assert_eq!(command_program("   "), "");
    assert_eq!(script_reference("./guard.sh"), "guard.sh");
    assert_eq!(script_reference("bin/guard.sh"), "bin/guard.sh");
}
