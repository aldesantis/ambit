use crate::errors::{Result, config_error};
use crate::model::reference::Reference;
use crate::util::string_enum;

pub const KIND_SEPARATOR: &str = ":";

pub const CATALOG_SEPARATOR: &str = "/";

string_enum! {
    pub enum ItemKind {
        Pack => "pack",
        Skill => "skill",
        Mcp => "mcp",
        Hook => "hook",
    }
}

pub const ITEM_KINDS: &[ItemKind] = ItemKind::ALL;

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

fn spellings(text: &str) -> String {
    ITEM_KINDS
        .iter()
        .map(|kind| format!("`{kind}{KIND_SEPARATOR}{text}`"))
        .collect::<Vec<_>>()
        .join(", ")
}
