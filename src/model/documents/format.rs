use std::path::Path;

use indexmap::IndexSet;

use crate::errors::{Result, config_error};
use crate::util::fs;
use crate::util::json::JsonValue;
use crate::util::string_enum;

string_enum! {
    pub enum DocumentFormat {
        Json => "json",
        Jsonc => "jsonc",
        Toml => "toml",
    }
}

pub const DOCUMENT_FORMATS: &[DocumentFormat] = DocumentFormat::ALL;

string_enum! {
    pub enum DocumentShape {
        Map => "map",
        Array => "array",
        List => "list",
    }
}

pub const DOCUMENT_SHAPES: &[DocumentShape] = DocumentShape::ALL;

#[derive(Clone, Debug, PartialEq)]
pub struct ConfigEntry {
    pub key: String,
    pub value: JsonValue,
}

pub trait DocumentDriver {
    fn section_keys(
        &self,
        text: Option<&str>,
        section: &str,
        file: &str,
    ) -> Result<IndexSet<String>>;

    fn merge_section(
        &self,
        text: Option<&str>,
        section: &str,
        entries: &[ConfigEntry],
        file: &str,
    ) -> Result<String>;

    fn entry_matches(
        &self,
        text: Option<&str>,
        section: &str,
        entry: &ConfigEntry,
        file: &str,
    ) -> Result<bool>;

    fn remove_keys(
        &self,
        text: Option<&str>,
        section: &str,
        keys: &[String],
        file: &str,
    ) -> Result<Option<String>>;
}

pub fn read_document_text(target: &Path, file: &str) -> Result<Option<String>> {
    fs::read_text_opt(target).map_err(|error| {
        config_error(
            format!("cannot read {file}"),
            [
                fs::io_message(&error, target),
                format!(
                    "make {} readable, or move it aside so ambit can write a fresh one",
                    target.display()
                ),
            ],
        )
    })
}

pub fn managed_key(section: &str, key: &str) -> String {
    format!("{section}.{key}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::ExitCode;

    #[test]
    fn reads_an_absent_file_as_no_document() {
        let dir = tempfile::tempdir().expect("a tempdir");

        assert_eq!(
            read_document_text(&dir.path().join("absent.json"), "absent.json"),
            Ok(None)
        );
    }

    #[test]
    fn reads_a_present_file() {
        let dir = tempfile::tempdir().expect("a tempdir");
        let target = dir.path().join("present.json");
        fs::write_text(&target, "{}\n").expect("written");

        assert_eq!(
            read_document_text(&target, "present.json"),
            Ok(Some("{}\n".to_owned()))
        );
    }

    #[test]
    fn refuses_a_file_it_cannot_read_rather_than_treating_it_as_absent() {
        let dir = tempfile::tempdir().expect("a tempdir");

        let error = read_document_text(dir.path(), "dir.json").expect_err("a refusal");

        assert_eq!(error.code, ExitCode::Config);
        assert_eq!(error.message, "cannot read dir.json");
        assert_eq!(error.detail.len(), 2);
    }
}
