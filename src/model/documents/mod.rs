//! The document drivers, keyed by the format a harness profile names and the shape of its section.

mod format;
mod json;
mod json_array;
mod jsonc;
mod toml;

pub use format::{
    ConfigEntry, DOCUMENT_FORMATS, DOCUMENT_SHAPES, DocumentDriver, DocumentFormat, DocumentShape,
    is_record, managed_key, read_document_text,
};
pub use json::{JsonDriver, parse_json_document, serialize_json_document};
pub use json_array::{
    ArraySectionDriver, DIGEST_LENGTH, array_entry_key, array_section_driver, entry_digest,
};
pub use jsonc::JsoncDriver;
pub use toml::TomlDriver;

pub use crate::util::json::{JsonObject, structurally_equal};

use crate::errors::Result;

/// The driver for one format and section shape.
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
    let _ = (format, shape, root_defaults);
    todo!("port model/documents/index.ts:driverFor")
}
