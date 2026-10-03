//! Locating a Markdown document's frontmatter block: a hand port of gray-matter 4.0.3's.
//!
//! Only gray-matter's location logic is ported, quirks included, so a document splits where the
//! TypeScript build split it. Its parsing is not: the block's contents go through ambit's own YAML
//! rules, so no laxer parser ever sees ambit's YAML.

use crate::errors::{Result, config_error};
use crate::util::text::{is_js_whitespace, js_trim};

const DELIMITER: &str = "---";

const FRONTMATTER_LANGUAGE: &str = "yaml";

/// A Markdown document cut into its frontmatter block and the bytes around it.
///
/// The three pieces concatenate back to the document exactly, so an edit can rewrite `block` and
/// leave the body byte-for-byte alone. `block` carries no surrounding blank lines, because the YAML
/// parser does not preserve those; they survive as bytes in `open`/`close` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrontmatterSplit {
    /// The opening delimiter, its language tag if any, and every blank line under it.
    pub open: String,
    /// The YAML block itself.
    pub block: String,
    /// The block's trailing blank lines, the closing delimiter, and the whole Markdown body.
    pub close: String,
}

/// What gray-matter reports about a document.
struct Matter<'t> {
    /// The raw block, `""` when there is none.
    raw: &'t str,
    /// The declared language, `yaml` when none is declared.
    language: &'t str,
    /// The block holds nothing but whitespace and comments.
    is_empty: bool,
}

/// gray-matter's `parseMatter`, reduced to what ambit reads from it.
fn matter(text: &str) -> Matter<'_> {
    let none = Matter {
        raw: "",
        language: FRONTMATTER_LANGUAGE,
        is_empty: false,
    };
    let content = text.strip_prefix('\u{FEFF}').unwrap_or(text);

    // If the next character after the opening delimiter is a character from the delimiter, it is
    // not a frontmatter delimiter.
    let Some(after) = content.strip_prefix(DELIMITER) else {
        return none;
    };

    if after.starts_with('-') {
        return none;
    }

    let mut rest = after;
    let mut language = FRONTMATTER_LANGUAGE;
    // `str.slice(0, str.search(/\r?\n/))`: with no newline, the search is -1 and the slice drops
    // the last character.
    let raw_language = match rest.find('\n') {
        Some(newline) => rest[..newline]
            .strip_suffix('\r')
            .unwrap_or(&rest[..newline]),
        None => rest
            .char_indices()
            .last()
            .map_or("", |(last, _)| &rest[..last]),
    };
    let name = js_trim(raw_language);

    if !name.is_empty() {
        language = name;
        rest = &rest[raw_language.len()..];
    }

    let close = rest.find("\n---").unwrap_or(rest.len());
    let raw = &rest[..close];

    Matter {
        raw,
        language,
        is_empty: is_blank(raw),
    }
}

/// gray-matter's `isEmpty`: the block minus `^\s*#[^\n]+` lines, trimmed, is empty.
///
/// Only whitespace and comment text can be removed, so a block is blank exactly when every line,
/// once its leading whitespace is skipped, is empty or a comment. A bare `#` is not one: the
/// pattern needs a character after it.
fn is_blank(raw: &str) -> bool {
    raw.split('\n').all(|line| {
        let line = line.trim_start_matches(is_js_whitespace);

        line.is_empty() || line.strip_prefix('#').is_some_and(|rest| !rest.is_empty())
    })
}

/// gray-matter's engine lookup for a declared language.
fn registered(language: &str) -> bool {
    matches!(language, "yaml" | "json" | "javascript")
        || matches!(
            language.to_lowercase().as_str(),
            "yaml" | "yml" | "js" | "javascript"
        )
}

/// Finds the frontmatter block of a Markdown document (`SKILL.md`'s, in practice).
///
/// `text` is the whole document, frontmatter included; `file` is how it is named in error messages.
///
/// # Errors
///
/// Exit 2 if there is no frontmatter block, or it is empty, or it is not YAML.
pub fn split_frontmatter(text: &str, file: &str) -> Result<FrontmatterSplit> {
    let document = if text.is_empty() {
        Matter {
            raw: "",
            language: FRONTMATTER_LANGUAGE,
            is_empty: false,
        }
    } else {
        matter(text)
    };

    // `is_empty` distinguishes a block holding nothing from a document with no block at all: for
    // `---` immediately followed by `---`, gray-matter reports both, and the former is the useful
    // thing to say.
    if document.is_empty {
        return Err(config_error(
            format!("{file} has an empty frontmatter block"),
            [
                "expected a YAML mapping between the `---` delimiters",
                "add the keys this format requires",
            ],
        ));
    }

    if document.raw.is_empty() {
        return Err(config_error(
            format!("{file} has no frontmatter block"),
            [
                "expected the document to open with a `---` delimited YAML block",
                "add one, starting on the first line",
            ],
        ));
    }

    // Reached only by an unrecognized language tag, which gray-matter rejects on its own terms.
    if !registered(document.language) {
        return Err(config_error(
            format!("cannot read the frontmatter of {file}"),
            [
                format!(
                    "gray-matter engine \"{}\" is not registered",
                    document.language
                ),
                "write it as a plain `---` delimited YAML block".to_owned(),
            ],
        ));
    }

    if document.language != FRONTMATTER_LANGUAGE {
        return Err(config_error(
            format!(
                "{file} declares its frontmatter as \"{}\"",
                document.language
            ),
            [
                "ambit reads frontmatter as YAML".to_owned(),
                format!(
                    "remove the language tag after the opening `---`, or write `---{FRONTMATTER_LANGUAGE}`"
                ),
            ],
        ));
    }

    // The raw block sits between the opening delimiter and the newline that begins the closing one,
    // so the two ends of the document are simply what remains around it. Located by search rather
    // than by arithmetic over the delimiter's length, which also keeps a leading byte-order mark on
    // the `open` side where it belongs.
    let raw = document.raw;
    let block_start = text.find(raw).unwrap_or(0);
    let leading = raw.len() - raw.trim_start_matches('\n').len();
    let inner = &raw[leading..];
    let trailing = inner.len() - inner.trim_end_matches('\n').len();

    Ok(FrontmatterSplit {
        open: format!("{}{}", &text[..block_start], &raw[..leading]),
        block: inner[..inner.len() - trailing].to_owned(),
        close: format!(
            "{}{}",
            &inner[inner.len() - trailing..],
            &text[block_start + raw.len()..]
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concatenates_back_to_the_document() {
        let text = "\u{FEFF}---\n\nname: a\n\n---\n# Body\n";
        let split = split_frontmatter(text, "SKILL.md").unwrap();

        assert_eq!(split.open, "\u{FEFF}---\n\n");
        assert_eq!(split.block, "name: a");
        assert_eq!(split.close, "\n\n---\n# Body\n");
        assert_eq!(
            format!("{}{}{}", split.open, split.block, split.close),
            text
        );
    }

    #[test]
    fn runs_an_unclosed_block_to_the_end() {
        let split = split_frontmatter("---\nname: a\n", "SKILL.md").unwrap();

        assert_eq!(split.block, "name: a");
        assert_eq!(split.close, "\n");
    }

    #[test]
    fn reads_a_yaml_language_tag() {
        let split = split_frontmatter("---yaml\nname: a\n---\n", "SKILL.md").unwrap();

        assert_eq!(split.open, "---yaml\n");
        assert_eq!(split.block, "name: a");
    }

    #[test]
    fn treats_four_dashes_as_no_block() {
        assert_eq!(
            split_frontmatter("----\nname: a\n---\n", "SKILL.md")
                .unwrap_err()
                .message,
            "SKILL.md has no frontmatter block"
        );
    }

    #[test]
    fn names_an_unregistered_engine() {
        let error = split_frontmatter("---toml\nname = \"a\"\n---\n", "SKILL.md").unwrap_err();

        assert_eq!(
            error.detail[0],
            "gray-matter engine \"toml\" is not registered"
        );
    }
}
