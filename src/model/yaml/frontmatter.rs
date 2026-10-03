//! Locating a Markdown document's frontmatter block: a hand port of gray-matter 4.0.3's.

use crate::errors::Result;

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

/// Finds the frontmatter block of a Markdown document (`SKILL.md`'s, in practice).
///
/// `text` is the whole document, frontmatter included; `file` is how it is named in error messages.
///
/// # Errors
///
/// Exit 2 if there is no frontmatter block, or it is empty, or it is not YAML.
pub fn split_frontmatter(text: &str, file: &str) -> Result<FrontmatterSplit> {
    let _ = (text, file);
    todo!("port model/yaml.ts:splitFrontmatter")
}
