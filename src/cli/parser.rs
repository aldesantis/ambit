//! Argv parsing: a hand port of the subset of commander 15's `_parseCommand` and `parseOptions`
//! ambit uses, with commander's error precedence and wording.
//!
//! The surface is built once per run from [`CommandSpec`]s into a [`Command`] tree shaped the way
//! commander's was: the program at the root with `--version` and positional options, one child per
//! spec, each with the help option, and a group's children below it.
//!
//! Precedence, as commander has it, for one command level:
//! 1. Errors found while scanning options, in argv order: a value-taking option at the end of
//!    argv, a value outside an option's choices, and `--version` (which prints and exits).
//! 2. Dispatch to a subcommand named by the first operand.
//! 3. `--help` anywhere among the unrecognized words.
//! 4. Conflicting options.
//! 5. The first unknown option.
//! 6. Missing, then excess, positional arguments; or an unknown command at a level with no action.
//!
//! Short flags, option-argument variadics, environment-variable options and defaults are
//! commander features ambit never declares, and are not ported.

use std::sync::LazyLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::cli::commands::{
    CommandOptions, CommandSpec, DRY_RUN, OptionSpec, OptionValue, global_options,
};
use crate::cli::suggest::suggest_similar;

/// The program's name, in usage lines.
pub const PROGRAM_NAME: &str = "ambit";

/// The program's description, under its usage line.
pub const PROGRAM_DESCRIPTION: &str =
    "a deterministic dependency manager for AI-agent capabilities";

/// The flag that prints usage, on every command.
pub const HELP_FLAG: &str = "--help";

/// The help flag's description.
pub const HELP_DESCRIPTION: &str = "show usage";

/// What follows every usage error on stderr.
pub const HELP_AFTER_ERROR: &str = "(run `ambit --help` for usage)";

/// One declared option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opt {
    /// As declared, and as quoted in errors and help: `--format <format>`.
    pub flags: String,
    /// `--format`.
    pub long: String,
    /// commander's attribute name, the key in [`CommandOptions`]: `dryRun`.
    pub attr: String,
    /// Whether the option takes a value (`<value>` in its flags).
    pub takes_value: bool,
    pub description: String,
    pub choices: Option<Vec<String>>,
    pub repeatable: bool,
    /// Attribute names this option cannot be given with.
    pub conflicts: Vec<String>,
}

impl Opt {
    fn from_spec(spec: &OptionSpec) -> Self {
        let mut words = spec.flags.split(' ');
        let long = words.next().unwrap_or_default().to_owned();
        let takes_value = words.next().is_some_and(|word| word.starts_with('<'));
        let attr = camelcase(long.trim_start_matches('-'));

        Self {
            flags: spec.flags.to_owned(),
            long,
            attr,
            takes_value,
            description: spec.description.to_owned(),
            choices: spec
                .choices
                .map(|choices| choices.iter().map(|&choice| choice.to_owned()).collect()),
            repeatable: spec.repeatable,
            conflicts: spec.conflicts.iter().map(|&name| name.to_owned()).collect(),
        }
    }
}

/// One declared positional argument.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Arg {
    pub name: String,
    pub required: bool,
    pub variadic: bool,
    pub description: String,
}

impl Arg {
    fn from_spec(spec: &str, description: &str) -> Self {
        let required = spec.starts_with('<');
        // Specs are declared in source as `<name>` or `[name]`.
        let inner = &spec[1..spec.len() - 1];
        let (name, variadic) = match inner.strip_suffix("...") {
            Some(name) => (name, true),
            None => (inner, false),
        };

        Self {
            name: name.to_owned(),
            required,
            variadic,
            description: description.to_owned(),
        }
    }

    /// `<name>`, `[name...]`.
    pub fn human_readable(&self) -> String {
        let name = format!("{}{}", self.name, if self.variadic { "..." } else { "" });

        if self.required {
            format!("<{name}>")
        } else {
            format!("[{name}]")
        }
    }
}

/// One node of the command tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub args: Vec<Arg>,
    /// Declared options, not including the help option every command also takes.
    pub options: Vec<Opt>,
    pub commands: Vec<Command>,
    /// The ancestors' names, outermost first, for the usage line.
    pub ancestors: Vec<String>,
    /// The words a user types to reach this command, below the program: the key its handler and
    /// rule are filed under. Empty for the program.
    pub key: String,
    /// The version `--version` prints. Only the program has one.
    pub version: Option<String>,
    /// Whether the command has an action: every command but the program. A group's action prints
    /// its usage.
    pub has_action: bool,
    /// Whether the command acts, as opposed to only holding other commands.
    pub acts: bool,
}

impl Command {
    fn find_command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.name == name)
    }

    fn find_option(&self, arg: &str) -> Option<&Opt> {
        self.options.iter().find(|option| option.long == arg)
    }

    /// Every option help lists, and the flags an unknown one is compared against: the declared
    /// ones, then the help option.
    pub fn visible_option_flags(&self) -> Vec<String> {
        self.options
            .iter()
            .map(|option| option.long.clone())
            .chain([HELP_FLAG.to_owned()])
            .collect()
    }
}

/// The command tree for a surface: the program, holding one command per spec.
pub fn program(specs: &[CommandSpec], version: &str) -> Command {
    let ancestors = vec![PROGRAM_NAME.to_owned()];

    Command {
        name: PROGRAM_NAME.to_owned(),
        description: PROGRAM_DESCRIPTION.to_owned(),
        args: Vec::new(),
        options: vec![Opt::from_spec(&OptionSpec {
            flags: "--version",
            description: "print the ambit version",
            choices: None,
            repeatable: false,
            conflicts: &[],
        })],
        commands: specs
            .iter()
            .map(|spec| build(spec, &ancestors, &[]))
            .collect(),
        ancestors: Vec::new(),
        key: String::new(),
        version: Some(version.to_owned()),
        has_action: false,
        acts: false,
    }
}

/// Builds the command for one spec, recursing into a group's subcommands.
///
/// `trail` is the enclosing group's words, so a nested command is keyed by the whole invocation
/// (`<group> <command>`) rather than by the leaf.
fn build(spec: &CommandSpec, ancestors: &[String], trail: &[String]) -> Command {
    let mut words = trail.to_vec();

    words.push(spec.name.to_owned());

    let acts = spec.subcommands.is_none();
    let mut options: Vec<Opt> = spec.options.iter().map(Opt::from_spec).collect();

    if spec.mutating {
        options.push(Opt::from_spec(&DRY_RUN));
    }

    // A group takes no flags of its own: there is nothing for `--json` to shape when the answer is
    // a usage message, and a group sharing a flag with its children would silently claim it first.
    if acts {
        options.extend(
            global_options(spec.reads_project)
                .iter()
                .map(Opt::from_spec),
        );
    }

    let mut child_ancestors = ancestors.to_vec();

    child_ancestors.push(spec.name.to_owned());

    Command {
        name: spec.name.to_owned(),
        description: spec.summary.to_owned(),
        args: spec
            .args
            .iter()
            .map(|arg| Arg::from_spec(arg.spec, arg.description))
            .collect(),
        options,
        commands: spec
            .subcommands
            .iter()
            .flatten()
            .map(|child| build(child, &child_ancestors, &words))
            .collect(),
        ancestors: ancestors.to_vec(),
        key: words.join(" "),
        version: None,
        has_action: true,
        acts,
    }
}

/// `dry-run` → `dryRun`, as commander names an option's attribute.
fn camelcase(name: &str) -> String {
    let mut words = name.split('-');
    let mut out = words.next().unwrap_or_default().to_owned();

    for word in words {
        let mut chars = word.chars();

        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }

    out
}

/// A parse that ended before any action: what to print, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop<'c> {
    /// `--help`: this command's usage on stdout, exit 0.
    Help(&'c Command),
    /// A command that holds others was given nothing to do: its usage on stderr, exit 2.
    HelpError(&'c Command),
    /// `--version`: the version on stdout, exit 0.
    Version(String),
    /// A usage error: the message (which may span lines) and [`HELP_AFTER_ERROR`] on stderr,
    /// exit 2.
    Usage(String),
}

/// A parse that reached a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed<'c> {
    /// The command whose action runs, with the flags and positionals it was given.
    Action {
        command: &'c Command,
        options: CommandOptions,
        args: Vec<String>,
    },
    /// A command with no action accepted the invocation and has nothing to run.
    Nothing,
}

/// Parses `argv` (the words after the program name) against `program`.
///
/// # Errors
///
/// A [`Stop`] when the invocation ends in help, the version, or a usage error.
pub fn parse<'c>(program: &'c Command, argv: &[String]) -> Result<Parsed<'c>, Stop<'c>> {
    parse_command(program, Vec::new(), argv)
}

fn parse_command<'c>(
    command: &'c Command,
    operands: Vec<String>,
    unknown: &[String],
) -> Result<Parsed<'c>, Stop<'c>> {
    let scanned = parse_options(command, unknown)?;
    let mut operands = operands;

    operands.extend(scanned.operands);

    let unknown = scanned.unknown;
    let args: Vec<String> = operands.iter().chain(&unknown).cloned().collect();

    if let Some(sub) = operands
        .first()
        .and_then(|first| command.find_command(first))
    {
        return parse_command(sub, operands[1..].to_vec(), &unknown);
    }

    if !command.commands.is_empty() && args.is_empty() && !command.has_action {
        return Err(Stop::HelpError(command));
    }

    if unknown.iter().any(|arg| arg == HELP_FLAG) {
        return Err(Stop::Help(command));
    }

    check_conflicts(command, &scanned.values)?;

    let check_unknown = || match unknown.first() {
        Some(flag) => Err(unknown_option(command, flag)),
        None => Ok(()),
    };

    if command.has_action {
        check_unknown()?;
        check_arguments(command, &args)?;

        return Ok(Parsed::Action {
            command,
            options: CommandOptions(scanned.values),
            // With excess arguments refused, every argument given is one declared, and a variadic
            // one collects the rest: the flattened list is `args` itself.
            args,
        });
    }

    if !operands.is_empty() {
        if !command.commands.is_empty() {
            return Err(unknown_command(command, &args[0]));
        }

        check_unknown()?;
        check_arguments(command, &args)?;
    } else if !command.commands.is_empty() {
        check_unknown()?;

        return Err(Stop::HelpError(command));
    } else {
        check_unknown()?;
        check_arguments(command, &args)?;
    }

    Ok(Parsed::Nothing)
}

/// What scanning one level's words produced.
struct Scanned {
    values: IndexMap<String, OptionValue>,
    /// Words that are not options or option values.
    operands: Vec<String>,
    /// The first unknown option and every word after it, left for a subcommand to claim.
    unknown: Vec<String>,
}

fn maybe_option(arg: &str) -> bool {
    arg.len() > 1 && arg.starts_with('-')
}

/// A negative number is an operand, not an unknown option, in a command with no subcommands.
fn negative_number(arg: &str) -> bool {
    static NEGATIVE_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^-([0-9]+|[0-9]*\.[0-9]+)(e[+-]?[0-9]+)?$").expect("a valid regex")
    });

    NEGATIVE_NUMBER.is_match(arg)
}

fn parse_options<'c>(command: &'c Command, args: &[String]) -> Result<Scanned, Stop<'c>> {
    let mut values: IndexMap<String, OptionValue> = IndexMap::new();
    let mut operands: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut to_unknown = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        i += 1;

        if arg == "--" {
            let dest = if to_unknown {
                &mut unknown
            } else {
                &mut operands
            };

            if to_unknown {
                dest.push(arg.clone());
            }

            dest.extend(args[i..].iter().cloned());
            break;
        }

        if maybe_option(arg)
            && let Some(option) = command.find_option(arg)
        {
            if option.takes_value {
                let Some(value) = args.get(i) else {
                    return Err(Stop::Usage(format!(
                        "error: option '{}' argument missing",
                        option.flags
                    )));
                };

                i += 1;
                set_value(command, option, Some(value.as_str()), &mut values)?;
            } else {
                set_value(command, option, None, &mut values)?;
            }

            continue;
        }

        // A known value-taking long flag with its value attached: `--format=zip`.
        if let Some(index) = arg.find('=')
            && arg.starts_with("--")
            && index > 2
            && let Some(option) = command.find_option(&arg[..index])
            && option.takes_value
        {
            set_value(command, option, Some(&arg[index + 1..]), &mut values)?;
            continue;
        }

        // Not an option of this command: an operand, a subcommand's option, an unknown option, or
        // the help flag. After an unknown option every word is unknown, so a subcommand can claim
        // it.
        if !to_unknown
            && maybe_option(arg)
            && !(command.commands.is_empty() && negative_number(arg))
        {
            to_unknown = true;
        }

        // Positional options: this level's option parsing stops at a subcommand name.
        if operands.is_empty() && unknown.is_empty() && command.find_command(arg).is_some() {
            operands.push(arg.clone());
            unknown.extend(args[i..].iter().cloned());
            break;
        }

        if to_unknown {
            unknown.push(arg.clone());
        } else {
            operands.push(arg.clone());
        }
    }

    Ok(Scanned {
        values,
        operands,
        unknown,
    })
}

fn set_value<'c>(
    command: &'c Command,
    option: &Opt,
    value: Option<&str>,
    values: &mut IndexMap<String, OptionValue>,
) -> Result<(), Stop<'c>> {
    if option.attr == "version"
        && let Some(version) = &command.version
    {
        return Err(Stop::Version(version.clone()));
    }

    let Some(value) = value else {
        values.insert(option.attr.clone(), OptionValue::Flag);
        return Ok(());
    };

    if let Some(choices) = &option.choices
        && !choices.iter().any(|choice| choice == value)
    {
        return Err(Stop::Usage(format!(
            "error: option '{}' argument '{value}' is invalid. Allowed choices are {}.",
            option.flags,
            choices.join(", ")
        )));
    }

    if option.repeatable {
        match values.get_mut(&option.attr) {
            Some(OptionValue::List(list)) => list.push(value.to_owned()),
            _ => {
                values.insert(
                    option.attr.clone(),
                    OptionValue::List(vec![value.to_owned()]),
                );
            }
        }
    } else {
        values.insert(option.attr.clone(), OptionValue::Value(value.to_owned()));
    }

    Ok(())
}

/// The first declared option, in declaration order, given together with one it conflicts with.
fn check_conflicts<'c>(
    command: &'c Command,
    values: &IndexMap<String, OptionValue>,
) -> Result<(), Stop<'c>> {
    let defined: Vec<&Opt> = command
        .options
        .iter()
        .filter(|option| values.contains_key(&option.attr))
        .collect();

    for option in defined.iter().filter(|option| !option.conflicts.is_empty()) {
        if let Some(other) = defined
            .iter()
            .find(|other| option.conflicts.contains(&other.attr))
        {
            return Err(Stop::Usage(format!(
                "error: option '{}' cannot be used with option '{}'",
                option.flags, other.flags
            )));
        }
    }

    Ok(())
}

/// Missing required arguments first, then excess ones (unless the last argument is variadic).
fn check_arguments<'c>(command: &'c Command, args: &[String]) -> Result<(), Stop<'c>> {
    for (index, arg) in command.args.iter().enumerate() {
        if arg.required && index >= args.len() {
            return Err(Stop::Usage(format!(
                "error: missing required argument '{}'",
                arg.name
            )));
        }
    }

    if command.args.last().is_some_and(|arg| arg.variadic) {
        return Ok(());
    }

    if args.len() > command.args.len() {
        let expected = command.args.len();
        let plural = if expected == 1 { "" } else { "s" };
        let for_subcommand = if command.ancestors.is_empty() {
            String::new()
        } else {
            format!(" for '{}'", command.name)
        };

        return Err(Stop::Usage(format!(
            "error: too many arguments{for_subcommand}. Expected {expected} argument{plural} but got {}: {}.",
            args.len(),
            args.join(", ")
        )));
    }

    Ok(())
}

/// commander compares an unknown flag with the flags of this command and of each ancestor up to
/// the first with positional options. The program has them, and every command inherits them, so
/// that is this command's own flags.
fn unknown_option<'c>(command: &'c Command, flag: &str) -> Stop<'c> {
    let suggestion = if flag.starts_with("--") {
        suggest_similar(flag, &command.visible_option_flags())
    } else {
        String::new()
    };

    Stop::Usage(format!("error: unknown option '{flag}'{suggestion}"))
}

fn unknown_command<'c>(command: &'c Command, name: &str) -> Stop<'c> {
    let candidates: Vec<String> = command
        .commands
        .iter()
        .map(|command| command.name.clone())
        .collect();
    let suggestion = suggest_similar(name, &candidates);

    Stop::Usage(format!("error: unknown command '{name}'{suggestion}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_attributes_in_camelcase() {
        assert_eq!(camelcase("dry-run"), "dryRun");
        assert_eq!(camelcase("json"), "json");
        assert_eq!(camelcase("a-b-c"), "aBC");
    }

    #[test]
    fn reads_argument_specs() {
        let variadic = Arg::from_spec("[catalog...]", "");

        assert_eq!(
            (variadic.name.as_str(), variadic.required, variadic.variadic),
            ("catalog", false, true)
        );
        assert_eq!(variadic.human_readable(), "[catalog...]");

        let required = Arg::from_spec("<kind:name>", "");

        assert_eq!(
            (required.name.as_str(), required.required, required.variadic),
            ("kind:name", true, false)
        );
        assert_eq!(required.human_readable(), "<kind:name>");
    }

    #[test]
    fn reads_option_flags() {
        let option = Opt::from_spec(&OptionSpec {
            flags: "--project <dir>",
            description: "",
            choices: None,
            repeatable: false,
            conflicts: &[],
        });

        assert_eq!(option.long, "--project");
        assert_eq!(option.attr, "project");
        assert!(option.takes_value);
    }

    #[test]
    fn recognizes_negative_numbers() {
        assert!(negative_number("-1"));
        assert!(negative_number("-.5"));
        assert!(negative_number("-1e5"));
        assert!(!negative_number("-x"));
        assert!(!negative_number("--1"));
    }
}
