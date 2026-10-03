//! The program as a user reaches it: the surface, the parser's error contract, the flag-rule and
//! group seams, and `self-update`'s wiring. Each command's own behaviour is its own suite's.
#![allow(clippy::disallowed_methods)] // std::fs reads the recorded fixtures.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::cli::commands::{
    CommandHandlers, CommandRules, CommandSpec, ITEM_KIND_NAMES, command_specs, handler, rule,
};
use crate::cli::{CaptureIo, Io, handlers, rules, run_surface, run_with};
use crate::errors::{AmbitError, ExitCode};
use crate::model::requirement::ItemKind;
use crate::test_support::{CliResult, tempdir, test_env};
use crate::util::env::Env;

fn invoke_with(argv: &[&str], handlers: &CommandHandlers, rules: &CommandRules) -> CliResult {
    invoke_surface(&command_specs(), argv, handlers, rules)
}

fn invoke_surface(
    specs: &[CommandSpec],
    argv: &[&str],
    handlers: &CommandHandlers,
    rules: &CommandRules,
) -> CliResult {
    let argv: Vec<String> = argv.iter().map(|&arg| arg.to_owned()).collect();
    let mut io = CaptureIo::default();
    let code = run_surface(
        specs,
        &argv,
        Path::new("/"),
        &Env::new(),
        &mut io,
        handlers,
        rules,
    );

    captured(code, &io)
}

fn captured(code: ExitCode, io: &CaptureIo) -> CliResult {
    let joined = |lines: &[String]| lines.iter().map(|line| line.clone() + "\n").collect();

    CliResult {
        code,
        stdout: joined(&io.out),
        stderr: joined(&io.err),
    }
}

/// `ambit <argv>` with the shipped handlers and rules. Only for invocations that never reach a
/// handler: help, usage errors and rules.
fn invoke(argv: &[&str]) -> CliResult {
    invoke_with(argv, &handlers(), &rules())
}

/// The shipped handlers, with `name`'s replaced by one that records each visit and succeeds.
fn stub(name: &str) -> (CommandHandlers, Rc<RefCell<usize>>) {
    let visits = Rc::new(RefCell::new(0));
    let mut handlers = handlers();
    let counter = Rc::clone(&visits);

    handlers.insert(
        name.to_owned(),
        handler(move |_| {
            *counter.borrow_mut() += 1;
            Ok(ExitCode::Success)
        }),
    );

    (handlers, visits)
}

fn project_dir() -> PathBuf {
    std::env::temp_dir().join("ambit-cli-tests-project")
}

mod recorded_surface {
    use super::*;

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/cli")
    }

    fn read(name: &str) -> String {
        std::fs::read_to_string(fixtures().join(name))
            .unwrap_or_else(|error| panic!("{name}: {error}"))
    }

    /// The same matrix `tests/cli_surface.rs` runs through the binary, run in-process.
    #[test]
    fn reproduces_every_recorded_case_in_process() {
        let root = tempdir();
        let env = test_env(root.path());
        let cases: serde_json::Value =
            serde_json::from_str(&read("cases.json")).expect("cases.json is JSON");

        for (name, argv) in cases.as_object().expect("an object") {
            let argv: Vec<String> = argv
                .as_array()
                .expect("an array")
                .iter()
                .map(|arg| arg.as_str().expect("a string").to_owned())
                .collect();
            let mut io = CaptureIo::default();
            let code = run_with(&argv, root.path(), &env, &mut io, &handlers(), &rules());
            let result = captured(code, &io);
            let expected: i32 = read(&format!("{name}.code"))
                .trim()
                .parse()
                .expect("a code");

            assert_eq!(result.stdout, read(&format!("{name}.stdout")), "{name}");
            assert_eq!(result.stderr, read(&format!("{name}.stderr")), "{name}");
            assert_eq!(result.code.as_i32(), expected, "{name}");
        }
    }
}

mod the_command_surface {
    use super::*;

    #[test]
    fn declares_no_command_group_so_no_invocation_is_two_words() {
        for spec in command_specs() {
            assert!(spec.subcommands.is_none(), "{}", spec.name);
        }
    }

    #[test]
    fn does_not_answer_to_ambit_catalog_which_is_not_a_command_any_more() {
        for argv in [
            &["catalog"][..],
            &["catalog", "validate"],
            &["catalog", "dump"],
        ] {
            let result = invoke(argv);

            assert_eq!(result.code, ExitCode::Config, "{argv:?}");
            assert!(result.stderr.contains("unknown command 'catalog'"));
            assert_eq!(result.stdout, "");
        }
    }

    #[test]
    fn gives_every_command_the_same_global_flags_and_none_of_them_a_catalog_directory() {
        // `self-update` is the one command without `--project`, because its subject is the binary
        // and not a project. It keeps `--offline`, which it refuses with a message of its own.
        for spec in command_specs() {
            let result = invoke(&[spec.name, "--help"]);
            let help = &result.stdout;

            assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

            if spec.name == "self-update" {
                assert!(!help.contains("--project"), "{}", spec.name);
            } else {
                assert!(help.contains("--project <dir>"), "{}", spec.name);
            }

            assert!(help.contains("--json"), "{}", spec.name);
            assert!(help.contains("--offline"), "{}", spec.name);
            assert!(!help.contains("--catalog <dir>"), "{}", spec.name);

            if spec.name != "search" {
                assert!(!help.contains("--catalog"), "{}", spec.name);
            }
        }
    }

    #[test]
    fn reports_a_command_with_no_handler_as_unimplemented_naming_the_invocation() {
        let mut without_validate = handlers();

        without_validate.shift_remove("validate");

        let project = project_dir();
        let result = invoke_with(
            &["validate", "--project", &project.to_string_lossy()],
            &without_validate,
            &rules(),
        );

        assert_eq!(result.code, ExitCode::Internal, "{}", result.stderr);
        assert!(
            result
                .stderr
                .contains("command \"validate\" is not implemented yet")
        );
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn wires_a_handler_for_every_declared_command() {
        let handlers = handlers();

        for spec in command_specs() {
            assert!(handlers.contains_key(spec.name), "{}", spec.name);
        }

        assert_eq!(handlers.len(), command_specs().len());
    }

    #[test]
    fn offers_the_item_kinds_as_capability_choices() {
        let kinds: Vec<&str> = ItemKind::ALL.iter().map(|kind| kind.as_str()).collect();

        assert_eq!(kinds, ITEM_KIND_NAMES);
    }

    #[test]
    fn collects_a_repeatable_flag_in_the_order_typed() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let record = Rc::clone(&seen);
        let mut handlers = handlers();

        handlers.insert(
            "search".to_owned(),
            handler(move |ctx| {
                record.borrow_mut().push((
                    ctx.args.clone(),
                    ctx.options.list("capability").to_vec(),
                    ctx.options.list("catalog").to_vec(),
                ));
                Ok(ExitCode::Success)
            }),
        );

        let result = invoke_with(
            &[
                "search",
                "--capability",
                "skill",
                "foo*",
                "--capability=mcp",
                "--catalog",
                "a",
            ],
            &handlers,
            &rules(),
        );

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(
            seen.borrow().as_slice(),
            &[(
                vec!["foo*".to_owned()],
                vec!["skill".to_owned(), "mcp".to_owned()],
                vec!["a".to_owned()]
            )]
        );
    }

    #[test]
    fn flattens_a_variadic_argument_and_names_flags_in_camelcase() {
        let seen = Rc::new(RefCell::new(None));
        let record = Rc::clone(&seen);
        let mut handlers = handlers();

        handlers.insert(
            "update".to_owned(),
            handler(move |ctx| {
                *record.borrow_mut() = Some((
                    ctx.args.clone(),
                    ctx.options.flag("dryRun"),
                    ctx.options.flag("json"),
                ));
                Ok(ExitCode::Success)
            }),
        );

        let result = invoke_with(
            &["update", "a", "--dry-run", "b", "--", "--json"],
            &handlers,
            &CommandRules::new(),
        );

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(
            seen.borrow().clone(),
            Some((
                vec!["a".to_owned(), "b".to_owned(), "--json".to_owned()],
                true,
                false
            ))
        );
    }
}

/// A usage error leaves `run` as an exit code and prints through ambit's own output, exactly as
/// one of ambit's own errors does.
mod usage_errors_and_the_exit_code_contract {
    use super::*;

    #[test]
    fn returns_an_exit_code_for_an_unknown_flag_with_a_suggestion() {
        let project = project_dir();
        let result = invoke(&[
            "validate",
            "--jsonn",
            "--project",
            &project.to_string_lossy(),
        ]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(result.stderr.contains("error: unknown option '--jsonn'"));
        assert!(result.stderr.contains("(Did you mean --json?)"));
        // Refused before the handler ran, so nothing it would have printed reached stdout.
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn prints_a_commands_usage_on_help_at_exit_0() {
        let result = invoke(&["validate", "--help"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert!(result.stdout.contains("Usage: ambit validate"));
        assert_eq!(result.stderr, "");
    }

    #[test]
    fn returns_an_exit_code_for_a_missing_argument() {
        let project = project_dir();
        let result = invoke(&["why", "--project", &project.to_string_lossy()]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(
            result
                .stderr
                .contains("error: missing required argument 'kind:name'")
        );
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn refuses_quiet_and_no_color_which_no_command_accepts() {
        for flag in ["--quiet", "--no-color"] {
            let result = invoke(&["status", flag]);

            assert_eq!(result.code, ExitCode::Config);
            assert!(
                result
                    .stderr
                    .contains(&format!("error: unknown option '{flag}'"))
            );
            assert_eq!(result.stdout, "");
        }
    }

    #[test]
    fn prints_the_version_wherever_it_appears_among_the_programs_flags() {
        let result = invoke(&["--bogus", "--version"]);

        assert_eq!(result.code, ExitCode::Success);
        assert_eq!(result.stdout, format!("{}\n", crate::version::VERSION));
    }

    #[test]
    fn reads_a_command_word_after_the_options_terminator() {
        let (handlers, visits) = stub("status");
        let result = invoke_with(&["--", "status"], &handlers, &rules());

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(*visits.borrow(), 1);
    }

    #[test]
    fn prints_usage_to_stderr_when_given_only_the_options_terminator() {
        let result = invoke(&["--"]);

        assert_eq!(result.code, ExitCode::Config);
        assert_eq!(result.stdout, "");
        assert!(
            result
                .stderr
                .starts_with("Usage: ambit [options] [command]\n")
        );
    }

    #[test]
    fn takes_an_option_value_that_looks_like_a_flag() {
        let seen = Rc::new(RefCell::new(None));
        let record = Rc::clone(&seen);
        let mut handlers = handlers();

        handlers.insert(
            "status".to_owned(),
            handler(move |ctx| {
                *record.borrow_mut() = ctx.options.value("project").map(str::to_owned);
                Ok(ExitCode::Success)
            }),
        );

        let result = invoke_with(&["status", "--project", "--json"], &handlers, &rules());

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(seen.borrow().as_deref(), Some("--json"));
    }

    #[test]
    fn prints_an_ambit_error_in_its_own_shape_with_its_own_code() {
        let mut handlers = handlers();

        handlers.insert(
            "doctor".to_owned(),
            handler(|_| {
                Err(AmbitError::new(
                    ExitCode::Doctor,
                    "something is off",
                    ["fix it"],
                ))
            }),
        );

        let result = invoke_with(&["doctor"], &handlers, &rules());

        assert_eq!(result.code, ExitCode::Doctor);
        assert_eq!(result.stderr, "error: something is off\n       fix it\n");
    }

    #[test]
    fn passes_a_handlers_own_exit_code_through() {
        let mut handlers = handlers();

        handlers.insert("status".to_owned(), handler(|_| Ok(ExitCode::Drift)));

        assert_eq!(
            invoke_with(&["status"], &handlers, &rules()).code,
            ExitCode::Drift
        );
    }
}

/// The flag-rule seam: a rule refuses before the handler, and runs once, for the command it
/// belongs to. Each case injects its own rule, which is also how the seam is exercised by a
/// command that has none.
mod the_flag_rules_enforced_before_a_handler_runs {
    use super::*;

    #[test]
    fn declares_a_rule_only_for_the_three_commands_that_refuse_offline() {
        let mut keys: Vec<String> = rules().keys().cloned().collect();

        keys.sort();

        assert_eq!(keys, ["outdated", "self-update", "update"]);

        // `outdated` and `update` ask a remote the same question, so their refusal is one rule
        // twice; `self-update` refuses for a different reason and carries its own.
        let outdated = invoke(&["outdated", "--offline"]);
        let update = invoke(&["update", "--offline"]);
        let self_update = invoke(&["self-update", "--offline"]);

        assert_eq!(outdated.stderr, update.stderr);
        assert_ne!(outdated.stderr, self_update.stderr);
    }

    #[test]
    fn refuses_before_the_handler_in_ambits_own_message_shape() {
        let (handlers, visits) = stub("validate");
        let rules = CommandRules::from_iter([(
            "validate".to_owned(),
            rule(|_| {
                Err(AmbitError::new(
                    ExitCode::Config,
                    "refused by a rule (mcps/x.yml)",
                    ["do something else"],
                ))
            }),
        )]);
        let project = project_dir();
        let result = invoke_with(
            &["validate", "--project", &project.to_string_lossy()],
            &handlers,
            &rules,
        );

        assert_eq!(result.code, ExitCode::Config);
        assert!(result.stderr.contains("refused by a rule (mcps/x.yml)"));
        assert!(result.stderr.contains("do something else"));
        assert_eq!(*visits.borrow(), 0);
    }

    #[test]
    fn dispatches_to_the_handler_once_a_rule_accepts_the_flags_it_was_given() {
        let (handlers, visits) = stub("validate");
        let rules = CommandRules::from_iter([("validate".to_owned(), rule(|_| Ok(())))]);
        let project = project_dir();
        let result = invoke_with(
            &["validate", "--project", &project.to_string_lossy()],
            &handlers,
            &rules,
        );

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(*visits.borrow(), 1);
    }

    #[test]
    fn runs_a_rule_exactly_once_for_the_command_it_belongs_to() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let record = Rc::clone(&seen);
        let rules = CommandRules::from_iter([(
            "validate".to_owned(),
            rule(move |ctx| {
                record
                    .borrow_mut()
                    .push(ctx.options.value("project").map(str::to_owned));
                Ok(())
            }),
        )]);
        let (handlers, visits) = stub("validate");
        let project = project_dir().to_string_lossy().into_owned();
        let result = invoke_with(&["validate", "--project", &project], &handlers, &rules);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(seen.borrow().as_slice(), &[Some(project)]);
        assert_eq!(*visits.borrow(), 1);
    }
}

/// The group seam [`CommandSpec::subcommands`] is, which no command in the surface declares. Each
/// case builds its own surface holding a group, which is also how a second group would arrive.
mod the_nested_command_seam_no_command_uses {
    use super::*;

    fn group() -> Vec<CommandSpec> {
        vec![CommandSpec {
            name: "grp",
            summary: "a group",
            args: Vec::new(),
            options: Vec::new(),
            mutating: false,
            reads_project: true,
            subcommands: Some(vec![CommandSpec {
                name: "sub",
                summary: "a command",
                args: Vec::new(),
                options: Vec::new(),
                mutating: false,
                reads_project: true,
                subcommands: None,
            }]),
        }]
    }

    #[test]
    fn dispatches_a_nested_command_through_the_handler_keyed_by_the_whole_invocation() {
        let seen = Rc::new(RefCell::new(None));
        let record = Rc::clone(&seen);
        let handlers = CommandHandlers::from_iter([(
            "grp sub".to_owned(),
            handler(move |ctx| {
                *record.borrow_mut() = ctx.options.value("project").map(str::to_owned);
                Ok(ExitCode::Success)
            }),
        )]);
        let project = project_dir().to_string_lossy().into_owned();
        let result = invoke_surface(
            &group(),
            &["grp", "sub", "--project", &project],
            &handlers,
            &rules(),
        );

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        // Keyed by `grp sub` and not by `sub`, which is what makes two groups able to hold one
        // leaf name.
        assert_eq!(seen.borrow().as_deref(), Some(project.as_str()));
    }

    #[test]
    fn prints_usage_for_the_group_itself_which_has_no_action_of_its_own() {
        let result = invoke_surface(&group(), &["grp"], &handlers(), &rules());

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(
            result.stdout,
            "Usage: ambit grp [options] [command]\n\
             \n\
             a group\n\
             \n\
             Options:\n\
             \x20 --help         show usage\n\
             \n\
             Commands:\n\
             \x20 sub [options]  a command\n"
        );

        // And the group carries none of the flags its children do, rather than silently eating
        // them.
        for flag in ["--project", "--json", "--offline"] {
            let result = invoke_surface(&group(), &["grp", flag], &handlers(), &rules());

            assert_eq!(result.code, ExitCode::Config, "{flag}");
            assert!(
                result
                    .stderr
                    .contains(&format!("error: unknown option '{flag}'"))
            );
        }
    }

    #[test]
    fn reports_a_nested_usage_error_by_the_leaf_name() {
        let result = invoke_surface(&group(), &["grp", "sub", "extra"], &handlers(), &rules());

        assert_eq!(result.code, ExitCode::Config);
        assert_eq!(
            result.stderr,
            "error: too many arguments for 'sub'. Expected 0 arguments but got 1: extra.\n\
             (run `ambit --help` for usage)\n"
        );
    }

    #[test]
    fn reports_a_nested_command_with_no_handler_by_the_whole_invocation() {
        let result = invoke_surface(&group(), &["grp", "sub"], &CommandHandlers::new(), &rules());

        assert_eq!(result.code, ExitCode::Internal);
        assert!(
            result
                .stderr
                .contains("command \"grp sub\" is not implemented yet")
        );
    }
}

/// `ambit self-update` as a user reaches it: the spec, the rule, and the handler wired together.
mod ambit_self_update {
    use super::*;

    #[test]
    fn refuses_offline_before_it_gets_that_far() {
        let result = invoke(&["self-update", "--offline"]);

        assert_eq!(result.code, ExitCode::Network);
        assert!(
            result
                .stderr
                .contains("`--offline` cannot install a release")
        );
    }

    #[test]
    fn refuses_project_which_names_something_it_does_not_act_on() {
        let result = invoke(&["self-update", "--project", "."]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(result.stderr.contains("unknown option '--project'"));
    }

    #[test]
    fn takes_the_version_as_a_positional_since_version_prints_ambits_own() {
        let usage = invoke(&["self-update", "--help"]);

        assert_eq!(usage.code, ExitCode::Success);
        assert!(usage.stdout.contains("[version]"));
        assert!(!usage.stdout.contains("--version"));
    }
}

/// Help wraps to the width the output stream reports, as commander's does on a terminal.
mod help_width {
    use super::*;

    struct Sized {
        inner: CaptureIo,
        width: usize,
    }

    impl Io for Sized {
        fn stdout(&mut self, line: &str) {
            self.inner.stdout(line);
        }

        fn stderr(&mut self, line: &str) {
            self.inner.stderr(line);
        }

        fn help_width(&self, _error: bool) -> Option<usize> {
            Some(self.width)
        }
    }

    fn help_at(width: usize) -> String {
        let mut io = Sized {
            inner: CaptureIo::default(),
            width,
        };

        run_with(
            &["--help".to_owned()],
            Path::new("/"),
            &Env::new(),
            &mut io,
            &handlers(),
            &rules(),
        );

        io.inner.out.join("\n")
    }

    #[test]
    fn wraps_to_a_wider_terminal() {
        assert!(help_at(120).contains(
            "  status [options]                 compare what is installed against what resolve produces\n"
        ));
    }

    #[test]
    fn does_not_wrap_when_fewer_than_40_columns_remain() {
        // 35 columns of terms and spacing leave 25 for descriptions.
        let help = help_at(60);

        assert!(help.contains(
            "  validate [options]               validate everything this project configures, for CI\n"
        ));
    }
}
