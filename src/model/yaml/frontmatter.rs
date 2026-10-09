use gray_matter::engine::Engine;
use gray_matter::{Matter, ParsedEntity, Pod};

use crate::errors::{AmbitError, Result, config_error};

const DELIMITER: &str = "---";

struct Raw;

impl Engine for Raw {
    fn parse(_: &str) -> gray_matter::Result<Pod> {
        Ok(Pod::Null)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Frontmatter {
    pub block: String,
    pub line_offset: usize,
}

pub(super) fn frontmatter(text: &str, file: &str) -> Result<Frontmatter> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut lines = text.lines();

    if lines.next().map(str::trim_end) != Some(DELIMITER) {
        return Err(config_error(
            format!("{file} has no frontmatter block"),
            [
                "expected the document to open with a `---` delimited YAML block",
                "add one, starting on the first line",
            ],
        ));
    }

    if !lines.clone().any(|line| line.trim_end() == DELIMITER) {
        return Err(config_error(
            format!("{file} has an unclosed frontmatter block"),
            [
                "expected a closing `---` line after the YAML block",
                "add `---` on its own line where the block ends",
            ],
        ));
    }

    let parsed: ParsedEntity = Matter::<Raw>::new().parse(text).map_err(|error| {
        config_error(
            format!("cannot read the frontmatter of {file}"),
            [error.to_string(), "fix the frontmatter block".to_owned()],
        )
    })?;

    if parsed.matter.is_empty() {
        return Err(empty(file));
    }

    let raw: Vec<&str> = lines
        .take_while(|line| line.trim_end() != DELIMITER)
        .collect();
    let is_text = |line: &&str| !line.trim().is_empty();
    let first = raw.iter().position(is_text).unwrap_or(0);
    let last = raw.iter().rposition(is_text).unwrap_or(first);

    Ok(Frontmatter {
        block: raw[first..=last].join("\n"),
        line_offset: 1 + first,
    })
}

pub(super) fn empty(file: &str) -> AmbitError {
    config_error(
        format!("{file} has an empty frontmatter block"),
        [
            "expected a YAML mapping between the `---` delimiters",
            "add the keys this format requires",
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_lines_above_the_block() {
        let found = frontmatter("\u{FEFF}---\n\nname: a\n\n---\n# Body\n", "SKILL.md").unwrap();

        assert_eq!(found.block, "name: a");
        assert_eq!(found.line_offset, 2);
    }

    #[test]
    fn keeps_the_first_line_indented() {
        let found = frontmatter("---\n  name: a\n  description: b\n---\n", "SKILL.md").unwrap();

        assert_eq!(found.block, "  name: a\n  description: b");
        assert_eq!(found.line_offset, 1);
    }
}
