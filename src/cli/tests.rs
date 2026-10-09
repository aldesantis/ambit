#![allow(clippy::disallowed_methods)]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use clap::CommandFactory as _;

use crate::cli::commands::{Cli, CommandHandlers, CommandRules, ITEM_KIND_NAMES, handler, rule};
use crate::cli::{CaptureIo, Io, MAX_HELP_WIDTH, handlers, rules, run_with};
use crate::errors::{AmbitError, ExitCode};
use crate::model::requirement::ItemKind;
use crate::test_support::{CliResult, tempdir, test_env};
use crate::util::env::Env;

fn invoke_with(argv: &[&str], handlers: &CommandHandlers, rules: &CommandRules) -> CliResult {
    let argv: Vec<String> = argv.iter().map(|&arg| arg.to_owned()).collect();
    let mut io = CaptureIo::default();
    let code = run_with(&argv, Path::new("/"), &Env::new(), &mut io, handlers, rules);

    captured(code, &io)
}

fn command_names() -> Vec<String> {
    Cli::command()
        .get_subcommands()
        .map(|command| command.get_name().to_owned())
        .collect()
}

fn captured(code: ExitCode, io: &CaptureIo) -> CliResult {
    let joined = |lines: &[String]| lines.iter().map(|line| line.clone() + "\n").collect();

    CliResult {
        code,
        stdout: joined(&io.out),
        stderr: joined(&io.err),
    }
}

fn invoke(argv: &[&str]) -> CliResult {
    invoke_with(argv, &handlers(), &rules())
}

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
        for command in Cli::command().get_subcommands() {
            assert!(!command.has_subcommands(), "{}", command.get_name());
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
            assert!(result.stderr.contains("unrecognized subcommand 'catalog'"));
            assert_eq!(result.stdout, "");
        }
    }

    #[test]
    fn gives_every_command_the_same_global_flags_and_none_of_them_a_catalog_directory() {
        for name in command_names() {
            let result = invoke(&[&name, "--help"]);
            let help = &result.stdout;

            assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

            if name == "self-update" {
                assert!(!help.contains("--project"), "{name}");
            } else {
                assert!(help.contains("--project <dir>"), "{name}");
            }

            assert!(help.contains("--json"), "{name}");
            assert!(help.contains("--offline"), "{name}");
            assert!(!help.contains("--catalog <dir>"), "{name}");

            if name != "search" {
                assert!(!help.contains("--catalog"), "{name}");
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
        let names = command_names();

        for name in &names {
            assert!(handlers.contains_key(name), "{name}");
        }

        assert_eq!(handlers.len(), names.len());
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
    fn flattens_a_variadic_argument_and_keys_dry_run_in_camelcase() {
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
        assert!(
            result
                .stderr
                .contains("error: unexpected argument '--jsonn' found")
        );
        assert!(
            result
                .stderr
                .contains("tip: a similar argument exists: '--json'")
        );
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
            result.stderr.contains(
                "error: the following required arguments were not provided:\n  <kind:name>"
            )
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
                    .contains(&format!("error: unexpected argument '{flag}' found"))
            );
            assert_eq!(result.stdout, "");
        }
    }

    #[test]
    fn prints_the_version_on_stdout_at_exit_0() {
        for flag in ["--version", "-V"] {
            let result = invoke(&[flag]);

            assert_eq!(result.code, ExitCode::Success);
            assert_eq!(
                result.stdout,
                format!("ambit {}\n", crate::version::VERSION)
            );
        }
    }

    #[test]
    fn prints_usage_on_stdout_at_exit_0_when_given_no_command() {
        let result = invoke(&[]);

        assert_eq!(result.code, ExitCode::Success);
        assert!(result.stdout.contains("Usage: ambit <COMMAND>\n"));
        assert_eq!(result.stderr, "");
    }

    #[test]
    fn prints_usage_to_stderr_when_given_only_the_options_terminator() {
        let result = invoke(&["--"]);

        assert_eq!(result.code, ExitCode::Config);
        assert_eq!(result.stdout, "");
        assert!(result.stderr.contains("Usage: ambit <COMMAND>\n"));
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

mod the_flag_rules_enforced_before_a_handler_runs {
    use super::*;

    #[test]
    fn declares_a_rule_only_for_the_three_commands_that_refuse_offline() {
        let mut keys: Vec<String> = rules().keys().cloned().collect();

        keys.sort();

        assert_eq!(keys, ["outdated", "self-update", "update"]);

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
        assert!(
            result
                .stderr
                .contains("unexpected argument '--project' found")
        );
    }

    #[test]
    fn takes_the_version_as_a_positional_since_version_prints_ambits_own() {
        let usage = invoke(&["self-update", "--help"]);

        assert_eq!(usage.code, ExitCode::Success);
        assert!(usage.stdout.contains("[version]"));
        assert!(!usage.stdout.contains("--version"));
    }
}

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

        fn help_width(&self) -> Option<usize> {
            Some(self.width)
        }
    }

    fn help_at(width: usize) -> String {
        let mut io = Sized {
            inner: CaptureIo::default(),
            width,
        };

        run_with(
            &["search".to_owned(), "--help".to_owned()],
            Path::new("/"),
            &Env::new(),
            &mut io,
            &handlers(),
            &rules(),
        );

        io.inner.out.join("\n")
    }

    #[test]
    fn wraps_to_a_narrower_terminal() {
        let help = help_at(60);

        assert_ne!(help, help_at(MAX_HELP_WIDTH));
        assert!(
            help.lines().all(|line| line.chars().count() <= 60),
            "{help}"
        );
    }

    #[test]
    fn wraps_no_wider_than_the_maximum_on_a_wider_terminal() {
        assert_eq!(help_at(200), help_at(MAX_HELP_WIDTH));
    }
}
