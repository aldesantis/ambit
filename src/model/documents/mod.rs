mod format;
mod json;
mod json_array;
mod json_list;
mod jsonc;
mod toml;

pub use format::{
    ConfigEntry, DOCUMENT_FORMATS, DOCUMENT_SHAPES, DocumentDriver, DocumentFormat, DocumentShape,
    managed_key, read_document_text,
};
pub use json::JsonDriver;
pub use json_array::{array_entry_key, array_section_driver};
pub use json_list::{LIST_EVENT_FIELD, list_section_driver};
pub use jsonc::JsoncDriver;
pub use toml::TomlDriver;

pub use crate::util::json::JsonObject;

use crate::errors::{AmbitError, ExitCode, Result};

pub fn driver_for(
    format: DocumentFormat,
    shape: DocumentShape,
    root_defaults: Option<&JsonObject>,
) -> Result<Box<dyn DocumentDriver>> {
    match shape {
        DocumentShape::Map => Ok(match format {
            DocumentFormat::Json => Box::new(JsonDriver),
            DocumentFormat::Jsonc => Box::new(JsoncDriver),
            DocumentFormat::Toml => Box::new(TomlDriver),
        }),
        DocumentShape::Array => match format {
            DocumentFormat::Json => Ok(Box::new(array_section_driver(root_defaults))),
            DocumentFormat::Jsonc | DocumentFormat::Toml => Err(no_driver(format, shape)),
        },
        DocumentShape::List => match format {
            DocumentFormat::Json => Ok(Box::new(list_section_driver(root_defaults))),
            DocumentFormat::Jsonc | DocumentFormat::Toml => Err(no_driver(format, shape)),
        },
    }
}

fn no_driver(format: DocumentFormat, shape: DocumentShape) -> AmbitError {
    let article = if shape == DocumentShape::Array {
        "an"
    } else {
        "a"
    };

    AmbitError::new(
        ExitCode::Internal,
        format!("no {format} driver for {article} {shape}-shaped section"),
        [
            format!(
                "every harness file ambit writes {article} {shape}-shaped section into is JSON"
            ),
            "this is a bug in ambit; nothing a project can hold selects this pairing".to_owned(),
        ],
    )
}
