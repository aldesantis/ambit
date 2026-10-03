//! How ambit writes a scaffolded file that is documentation as much as configuration.
//!
//! A scaffold is not a template. Every value goes through [`emit_yaml`], and prose is added as
//! comments afterwards, so stripping the comment lines from a scaffolded file leaves exactly what
//! ambit would emit from the same values: sorted keys, quoting where a string could coerce,
//! byte-stable across runs and machines. Templating the file as text instead could drift into an
//! unsorted key or an unquoted `1e5` that the parser it is written for rejects, unnoticed until
//! someone ran the tool.
//!
//! Blocks are laid out in sorted-key order for the same reason, including a block shown commented
//! out: uncommenting it must leave the file sorted and produce valid YAML immediately.
//!
//! `ambit init` is the only caller. This stays a separate module because the emit-then-comment
//! rule is a property of how ambit writes a documented file, not of which file is being written, so
//! a future scaffold should reuse it rather than reinvent it as a template.

use crate::model::yaml::emit_yaml;
use crate::util::json::{JsonObject, JsonValue};
use crate::util::text::js_trim_end;

/// One commented block of a scaffolded file: prose, then at most one of the two YAML forms.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScaffoldBlock {
    /// Prose, one entry per emitted line. An empty entry is a bare `#` separator.
    pub comment: Vec<String>,
    /// Keys this block sets.
    pub values: Option<JsonObject>,
    /// Keys shown commented out, for something only the reader can supply.
    pub example: Option<JsonObject>,
}

/// Prefixes prose as YAML comments, leaving a blank entry as a bare `#` rather than `# `.
fn comment_out<S: AsRef<str>>(lines: &[S]) -> Vec<String> {
    lines
        .iter()
        .map(|line| match line.as_ref() {
            "" => "#".to_owned(),
            line => format!("# {line}"),
        })
        .collect()
}

fn emitted_lines(values: &JsonObject) -> Vec<String> {
    js_trim_end(&emit_yaml(&JsonValue::Object(values.clone())))
        .split('\n')
        .map(str::to_owned)
        .collect()
}

fn render_block(block: &ScaffoldBlock) -> String {
    let mut lines = comment_out(&block.comment);

    if let Some(example) = &block.example {
        lines.extend(comment_out(&emitted_lines(example)));
    }

    if let Some(values) = &block.values {
        lines.extend(emitted_lines(values));
    }

    lines.join("\n")
}

/// A scaffolded file, as bytes: each block separated by a blank line, with a trailing newline.
///
/// Pure and byte-stable: the output is a function of the blocks alone, so two runs on two machines
/// scaffold the same file.
pub fn render_scaffold(blocks: &[ScaffoldBlock]) -> String {
    let rendered: Vec<String> = blocks.iter().map(render_block).collect();

    format!("{}\n", rendered.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(value: JsonValue) -> JsonObject {
        match value {
            JsonValue::Object(object) => object,
            _ => unreachable!("an object literal"),
        }
    }

    #[test]
    fn renders_prose_then_the_example_then_the_values() {
        let text = render_scaffold(&[
            ScaffoldBlock {
                comment: vec![
                    "Which harnesses.".to_owned(),
                    String::new(),
                    "More.".to_owned(),
                ],
                values: Some(object(json!({"harnesses": ["claude"]}))),
                example: None,
            },
            ScaffoldBlock {
                comment: vec!["Catalogs.".to_owned()],
                values: None,
                example: Some(object(json!({"catalogs": [{"name": "company"}]}))),
            },
        ]);

        assert_eq!(
            text,
            "# Which harnesses.\n#\n# More.\nharnesses:\n  - claude\n\n# Catalogs.\n# catalogs:\n#   - name: company\n"
        );
    }
}
