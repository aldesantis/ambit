use crate::model::yaml::emit_yaml;
use crate::util::json::{JsonObject, JsonValue};
use crate::util::text::js_trim_end;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScaffoldBlock {
    pub comment: Vec<String>,
    pub values: Option<JsonObject>,
    pub example: Option<JsonObject>,
}

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
