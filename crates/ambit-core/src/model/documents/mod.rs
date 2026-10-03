//! The document drivers, keyed by the format a harness profile names and the shape of its section.

mod format;
mod json;
mod json_array;
mod jsonc;
mod toml;

pub use format::{
    ConfigEntry, DOCUMENT_FORMATS, DOCUMENT_SHAPES, DocumentDriver, DocumentFormat, DocumentShape,
    managed_key, read_document_text,
};
pub use json::JsonDriver;
pub use json_array::{array_entry_key, array_section_driver};
pub use jsonc::JsoncDriver;
pub use toml::TomlDriver;

pub use crate::util::json::JsonObject;

use crate::errors::{AmbitError, ExitCode, Result};

/// The driver for one format and section shape.
///
/// The map arm is exhaustive over [`DocumentFormat`], so adding a format is a compile error here
/// until a driver exists for it.
///
/// Callers holding an optional shape pass `shape.unwrap_or(DocumentShape::Map)`, exactly as an
/// absent format reads as `json`: both fields were added after artifacts were already being
/// recorded, and every one of those was a name-keyed JSON map.
///
/// `root_defaults` are root keys to seed where the document lacks them, for an array-shaped
/// section. `None` for every caller that only reads or removes: defaults belong to writing a
/// document.
///
/// # Errors
///
/// Exit 1 for a format with no array-section driver. Nothing in ambit plans one, so reaching it is
/// a bug, not something a person did. Falling back to the map driver would mean editing a hooks
/// file as if its arrays were tables.
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
        // Only JSON, because every file with an array-shaped section is JSON: Claude's
        // `settings.json`, Cursor's `hooks.json`, Codex's `hooks.json`. A pairing nothing supports
        // is a refusal rather than a driver that would write JSON into a `.toml`, so it cannot
        // silently corrupt a file.
        //
        // The driver is built per call rather than shared, because it carries the root defaults of
        // the harness whose file it edits (Cursor's `version: 1`), and those are the caller's to
        // name.
        DocumentShape::Array => match format {
            DocumentFormat::Json => Ok(Box::new(array_section_driver(root_defaults))),
            DocumentFormat::Jsonc | DocumentFormat::Toml => Err(AmbitError::new(
                ExitCode::Internal,
                format!("no {format} driver for an array-shaped section"),
                [
                    "every harness file ambit writes an array-shaped section into is JSON",
                    "this is a bug in ambit; nothing a project can hold selects this pairing",
                ],
            )),
        },
    }
}
