//! The vocabulary of naming: item kinds, and the separators an address and a subject use.
//!
//! This module imports nothing that could reach back into `catalog.rs` or `pattern.rs`, which is
//! why [`CATALOG_SEPARATOR`] lives here rather than beside `qualified_name`.

use crate::errors::{Result, config_error};
use crate::model::reference::Reference;
use crate::util::string_enum;

/// What separates a kind from a name on a command line.
pub const KIND_SEPARATOR: &str = ":";

/// What separates a catalog from a name in the address of a merged item.
///
/// `/` rather than `.`, so an address introduces no phantom level into the dotted namespace a
/// catalog already has: `company/core.a` is the item `core.a` in the catalog `company`.
pub const CATALOG_SEPARATOR: &str = "/";

string_enum! {
    /// Which of the bundle's namespaces a name belongs to.
    ///
    /// Declared in the order every report lists them. `pack` leads because a pack is what a project
    /// usually names, a capability whose job is to pull in the other three, so a report leading
    /// with packs shows what they expanded to, in the order the reader asked for things.
    pub enum ItemKind {
        Pack => "pack",
        Skill => "skill",
        Mcp => "mcp",
        Hook => "hook",
    }
}

/// The namespaces a bundle item can be in, in the order every report lists them.
///
/// Enumerated by the refusal for a `why` subject naming no namespace, by every report that groups
/// by namespace, and by the `requires` grammar, where a kind is the entry's key.
pub const ITEM_KINDS: &[ItemKind] = ItemKind::ALL;

/// The item one `<kind>:<name>` subject names.
///
/// Recognized by its kind rather than by the mere presence of a `:`, and split on the *first*
/// separator after that, so a name carrying one of its own survives the round trip
/// (`skill:odd:name` is the skill `odd:name`) while `server:fixture` is a bare name and gets the
/// refusal explaining the grammar.
///
/// `summary` is how the command names what it is missing (`` `why acme` does not say what to
/// explain ``), since that is the only half that could differ between two commands taking a
/// subject.
///
/// # Errors
///
/// Exit 2 for a bare name, which includes a `<prefix>:<name>` whose prefix is no namespace, or for
/// a namespace with no name after it.
pub fn parse_item_subject(text: &str, summary: &str) -> Result<Reference<ItemKind>> {
    let (kind_text, name) = match text.find(KIND_SEPARATOR) {
        Some(separator) => (
            &text[..separator],
            &text[separator + KIND_SEPARATOR.len()..],
        ),
        None => ("", ""),
    };

    let Some(kind) = ItemKind::parse(kind_text) else {
        return Err(config_error(
            summary,
            [
                "a bare name does not say what kind of thing it names".to_owned(),
                format!("write the subject as one of: {}", spellings(text)),
            ],
        ));
    };

    if name.is_empty() {
        return Err(config_error(
            summary,
            [
                format!("`{kind}{KIND_SEPARATOR}` names no item"),
                format!(
                    "write the subject as `<kind>{KIND_SEPARATOR}<name>`, one of: {}",
                    ITEM_KINDS
                        .iter()
                        .map(|kind| kind.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ],
        ));
    }

    Ok(Reference {
        kind,
        name: name.to_owned(),
    })
}

/// Every spelling of what was typed, as the refusal for a bare name offers them.
fn spellings(text: &str) -> String {
    ITEM_KINDS
        .iter()
        .map(|kind| format!("`{kind}{KIND_SEPARATOR}{text}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
