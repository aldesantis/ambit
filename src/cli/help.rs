//! Usage output: commander 15's `Help.formatHelp` and `wrap` layout, for a surface with no
//! colors, aliases, hidden items, option groups or sorting.

use crate::cli::parser::{Command, HELP_DESCRIPTION, HELP_FLAG};
use crate::util::text::{is_js_whitespace, js_len, js_trim_start, pad_end};

/// The width help wraps to when the stream is not a terminal.
pub const DEFAULT_HELP_WIDTH: usize = 80;

/// Below this many columns for a description, it is printed unwrapped.
const MIN_WIDTH_TO_WRAP: usize = 40;

/// Two spaces before every item, and two between a term and its description.
const ITEM_INDENT: usize = 2;
const SPACER_WIDTH: usize = 2;

/// `command`'s usage, wrapped to `width` columns, without a trailing newline.
pub fn format_help(command: &Command, width: usize) -> String {
    let options = visible_options(command);
    let term_width = options
        .iter()
        .map(|(term, _)| js_len(term))
        .chain(
            command
                .commands
                .iter()
                .map(|sub| js_len(&subcommand_term(sub))),
        )
        .chain(visible_arguments(command).map(|(term, _)| js_len(term)))
        .max()
        .unwrap_or(0);
    let item = |term: &str, description: &str| format_item(term, term_width, description, width);

    let mut output = vec![format!("Usage: {}", command_usage(command)), String::new()];

    if !command.description.is_empty() {
        output.push(box_wrap(&command.description, width));
        output.push(String::new());
    }

    let arguments: Vec<String> = visible_arguments(command)
        .map(|(term, description)| item(term, description))
        .collect();

    push_list(&mut output, "Arguments:", arguments);

    let options: Vec<String> = options
        .iter()
        .map(|(term, description)| item(term, description))
        .collect();

    push_list(&mut output, "Options:", options);

    let commands: Vec<String> = command
        .commands
        .iter()
        .map(|sub| item(&subcommand_term(sub), &sub.description))
        .collect();

    push_list(&mut output, "Commands:", commands);

    let text = output.join("\n");

    // The last list closes with a blank line, which the writer strips as commander's does.
    text.strip_suffix('\n').map(str::to_owned).unwrap_or(text)
}

fn push_list(output: &mut Vec<String>, heading: &str, items: Vec<String>) {
    if items.is_empty() {
        return;
    }

    output.push(heading.to_owned());
    output.extend(items);
    output.push(String::new());
}

/// `ambit install [options]`, `ambit search [options] <pattern>`.
fn command_usage(command: &Command) -> String {
    let mut usage: Vec<String> = vec!["[options]".to_owned()];

    if !command.commands.is_empty() {
        usage.push("[command]".to_owned());
    }

    usage.extend(
        command
            .args
            .iter()
            .map(crate::cli::parser::Arg::human_readable),
    );

    let mut words = command.ancestors.clone();

    words.push(format!("{} {}", command.name, usage.join(" ")));
    words.join(" ")
}

/// Arguments are listed only when one of them has a description; then all of them are.
fn visible_arguments(command: &Command) -> impl Iterator<Item = (&str, &str)> {
    let any_described = command.args.iter().any(|arg| !arg.description.is_empty());

    command
        .args
        .iter()
        .filter(move |_| any_described)
        .map(|arg| (arg.name.as_str(), arg.description.as_str()))
}

/// Each declared option's flags and description (with its choices), then the help option.
fn visible_options(command: &Command) -> Vec<(String, String)> {
    command
        .options
        .iter()
        .map(|option| {
            let description = match &option.choices {
                Some(choices) => {
                    let quoted: Vec<String> = choices
                        .iter()
                        .map(|choice| crate::util::json::stringify(&choice.as_str().into()))
                        .collect();
                    let extra = format!("(choices: {})", quoted.join(", "));

                    if option.description.is_empty() {
                        extra
                    } else {
                        format!("{} {extra}", option.description)
                    }
                }
                None => option.description.clone(),
            };

            (option.flags.clone(), description)
        })
        .chain([(HELP_FLAG.to_owned(), HELP_DESCRIPTION.to_owned())])
        .collect()
}

/// A command as its parent lists it: `search [options] <pattern>`.
fn subcommand_term(command: &Command) -> String {
    let args: Vec<String> = command
        .args
        .iter()
        .map(crate::cli::parser::Arg::human_readable)
        .collect();
    let mut term = command.name.clone();

    // commander's check counts declared options only, not the help option.
    if !command.options.is_empty() {
        term.push_str(" [options]");
    }

    if !args.is_empty() {
        term.push(' ');
        term.push_str(&args.join(" "));
    }

    term
}

/// Whether a description carries its own layout: a line break followed by indentation.
fn preformatted(text: &str) -> bool {
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\n'
            && chars
                .peek()
                .is_some_and(|&next| is_js_whitespace(next) && next != '\r' && next != '\n')
        {
            return true;
        }
    }

    false
}

/// One term and its description, the description wrapped into the columns right of the terms.
fn format_item(term: &str, term_width: usize, description: &str, help_width: usize) -> String {
    let indent = " ".repeat(ITEM_INDENT);

    if description.is_empty() {
        return format!("{indent}{term}");
    }

    let padded_term = pad_end(term, term_width);
    let remaining_width = help_width.saturating_sub(term_width + SPACER_WIDTH + ITEM_INDENT);
    let formatted = if remaining_width < MIN_WIDTH_TO_WRAP || preformatted(description) {
        description.to_owned()
    } else {
        box_wrap(description, remaining_width).replace(
            '\n',
            &format!("\n{}", " ".repeat(term_width + SPACER_WIDTH)),
        )
    };

    format!(
        "{indent}{padded_term}{}{}",
        " ".repeat(SPACER_WIDTH),
        formatted.replace('\n', &format!("\n{indent}"))
    )
}

/// Wraps `text` at whitespace to `width` columns, leaving it as is below
/// [`MIN_WIDTH_TO_WRAP`].
fn box_wrap(text: &str, width: usize) -> String {
    if width < MIN_WIDTH_TO_WRAP {
        return text.to_owned();
    }

    let mut wrapped: Vec<String> = Vec::new();

    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let chunks = chunks(line);
        let mut chunks = chunks.into_iter();
        let Some(first) = chunks.next() else {
            wrapped.push(String::new());
            continue;
        };
        let mut sum = first.to_owned();
        let mut sum_width = js_len(first);

        for chunk in chunks {
            let visible = js_len(chunk);

            if sum_width + visible <= width {
                sum.push_str(chunk);
                sum_width += visible;
                continue;
            }

            wrapped.push(std::mem::take(&mut sum));

            let next = js_trim_start(chunk);

            sum.push_str(next);
            sum_width = js_len(next);
        }

        wrapped.push(sum);
    }

    wrapped.join("\n")
}

/// `line.match(/[\s]*[^\s]+/g)`: each word with the whitespace before it. Trailing whitespace
/// belongs to no chunk.
fn chunks(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_word = false;

    for (index, c) in line.char_indices() {
        let space = is_js_whitespace(c);

        if in_word && space {
            out.push(&line[start..index]);
            start = index;
            in_word = false;
        } else if !space {
            in_word = true;
        }
    }

    if in_word {
        out.push(&line[start..]);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_with_their_leading_whitespace() {
        assert_eq!(chunks("  a bb  c "), vec!["  a", " bb", "  c"]);
        assert_eq!(chunks("   "), Vec::<&str>::new());
    }

    #[test]
    fn wraps_at_whitespace() {
        let text = "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj kkkk llll";

        assert_eq!(
            box_wrap(text, 40),
            "aaaa bbbb cccc dddd eeee ffff gggg hhhh\niiii jjjj kkkk llll"
        );
    }

    #[test]
    fn leaves_narrow_text_unwrapped() {
        let text = "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii";

        assert_eq!(box_wrap(text, 39), text);
    }

    #[test]
    fn detects_preformatted_descriptions() {
        assert!(preformatted("a\n  b"));
        assert!(!preformatted("a\nb"));
        assert!(!preformatted("a\n\nb"));
    }
}
