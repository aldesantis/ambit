//! The YAML emitter: yaml@2.9.0's `stringify` output for the subset ambit uses, so emitted files
//! stay byte-identical across releases.
//!
//! It reproduces `stringify` with these options, and each encodes a rule:
//!
//! - `sortMapEntries` makes the byte order a function of the keys rather than of whichever order a
//!   generator happened to build its object in: the property that lets a lock be diffed at all.
//! - `singleQuote: false` makes quoting consistent, so a value that needs quotes always gets the
//!   same ones and a diff never shows a changed quote style as a change.
//! - `blockQuote: false` and `lineWidth: 0` between them forbid every form of rewrapping: a long
//!   value stays on its line, and an awkward one is escaped inside double quotes rather than
//!   turned into a block scalar whose indentation carries meaning.
//! - `aliasDuplicateObjects: false` forbids anchors and aliases, so two entries that happen to
//!   hold equal values stay two entries instead of one and a back-reference.
//! - The core schema matches the parser's, which is what makes the two halves agree about when a
//!   string needs quoting: an emitter on a laxer schema would leave `1e5` bare for a parser that
//!   reads it as a float.
//!
//! Only the paths those options leave reachable are implemented. A JSON value builds no flow
//! collection, carries no comments, and never needs a block scalar, so none of that is here.

use std::sync::LazyLock;

use regex::Regex;

use super::schema;
use crate::util::cmp::js_cmp;
use crate::util::json::{self, JsonValue};
use crate::util::text::js_len;

const INDENT_STEP: &str = "  ";

/// yaml's `doubleQuotedMinMultiLineLength`: a shorter double-quoted string keeps `\n` escapes.
const DOUBLE_QUOTED_MIN_MULTI_LINE_LENGTH: usize = 40;

/// A key longer than this is written as an explicit `? key`.
const MAX_IMPLICIT_KEY_LENGTH: usize = 1024;

/// Control characters force double quotes (Rust strings hold no unpaired surrogates).
static CONTROL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\x00-\x08\x0b-\x1f\x7f-\u{9f}]").expect("a valid control-character pattern")
});

/// What a plain scalar must not look like:
/// - `-` or `?` alone,
/// - start with an indicator character (except `?:-`) or with `? ` / `- `,
/// - `\n `, `: ` or ` \n` anywhere,
/// - `#` not preceded by a non-space character,
/// - end with a space or `:`.
static NOT_PLAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^[\n\t ,\[\]{}#&*!|>'"%@`]|^[?-]$|^[?-][ \t]|[\n:][ \t]|[ \t]\n|[\n\t ]#|[\n\t :]$"#,
    )
    .expect("a valid plain-scalar pattern")
});

/// Also catches lines starting with `%`, which a YAML 1.1 parser would take for a directive.
static DOCUMENT_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^(?:%|---|\.\.\.)").expect("a valid document-marker pattern")
});

#[derive(Clone)]
struct Context {
    indent: String,
    implicit_key: bool,
}

/// Renders a document as the bytes ambit writes: keys sorted at every depth, strings that would
/// otherwise coerce double-quoted, no anchors, no aliases, no rewrapping, and a trailing newline.
///
/// Byte-stable by construction: the output is a function of the values alone, so the same inputs
/// produce the same file and a lock that has not changed shows as no diff.
pub fn emit_yaml(document: &JsonValue) -> String {
    let context = Context {
        indent: String::new(),
        implicit_key: false,
    };

    format!("{}\n", node(document, &context))
}

fn node(value: &JsonValue, context: &Context) -> String {
    match value {
        JsonValue::Null => "null".to_owned(),
        JsonValue::Bool(true) => "true".to_owned(),
        JsonValue::Bool(false) => "false".to_owned(),
        JsonValue::Number(number) => number_text(number),
        JsonValue::String(text) => string(text, context),
        JsonValue::Array(items) => sequence(items, context),
        JsonValue::Object(object) => mapping(object, context),
    }
}

/// `JSON.stringify` of the number, except that negative zero keeps its sign.
fn number_text(number: &serde_json::Number) -> String {
    match number.as_f64() {
        Some(f) if f == 0.0 && f.is_sign_negative() => "-0".to_owned(),
        _ => json::format_number(number),
    }
}

fn sequence(items: &[JsonValue], context: &Context) -> String {
    if items.is_empty() {
        return "[]".to_owned();
    }

    let item_context = Context {
        indent: format!("{}  ", context.indent),
        implicit_key: context.implicit_key,
    };

    items
        .iter()
        .map(|item| format!("- {}", node(item, &item_context)))
        .collect::<Vec<_>>()
        .join(&format!("\n{}", context.indent))
}

fn mapping(object: &json::JsonObject, context: &Context) -> String {
    if object.is_empty() {
        return "{}".to_owned();
    }

    let mut entries: Vec<(&String, &JsonValue)> = object.iter().collect();

    entries.sort_by(|a, b| js_cmp(a.0, b.0));

    entries
        .into_iter()
        .map(|(key, value)| pair(key, value, &context.indent))
        .collect::<Vec<_>>()
        .join(&format!("\n{}", context.indent))
}

/// yaml's `stringifyPair`, for a string key.
fn pair(key: &str, value: &JsonValue, indent: &str) -> String {
    let inner = format!("{indent}{INDENT_STEP}");
    let key_text = string(
        key,
        &Context {
            indent: inner.clone(),
            implicit_key: true,
        },
    );
    let explicit_key = js_len(&key_text) > MAX_IMPLICIT_KEY_LENGTH;
    let head = if explicit_key {
        format!("? {key_text}\n{indent}:")
    } else {
        format!("{key_text}:")
    };
    let value_context = Context {
        indent: inner.clone(),
        implicit_key: false,
    };
    let value_text = node(value, &value_context);
    let separator = match value {
        JsonValue::Array(items) if !explicit_key && !items.is_empty() => format!("\n{inner}"),
        JsonValue::Object(object) if !explicit_key && !object.is_empty() => format!("\n{inner}"),
        _ => " ".to_owned(),
    };

    format!("{head}{separator}{value_text}")
}

/// yaml's `stringifyString` with `defaultStringType: PLAIN`.
fn string(value: &str, context: &Context) -> String {
    if CONTROL.is_match(value) {
        return double_quoted(value, context);
    }

    plain(value, context)
}

/// yaml's `plainString`, falling back to double quotes wherever it would reach for quotes or a
/// block scalar (`blockQuote: false` turns every block scalar into a quoted string).
fn plain(value: &str, context: &Context) -> String {
    if context.implicit_key && value.contains('\n') {
        return double_quoted(value, context);
    }

    if NOT_PLAIN.is_match(value) {
        return double_quoted(value, context);
    }

    if !context.implicit_key && value.contains('\n') {
        return double_quoted(value, context);
    }

    if DOCUMENT_MARKER.is_match(value)
        && (context.indent.is_empty() || (context.implicit_key && context.indent == INDENT_STEP))
    {
        return double_quoted(value, context);
    }

    // Verify that the output will be parsed as a string, as plain numbers and booleans are read
    // with those types in YAML 1.2 (`42`, `true`, `0.9e-3`).
    if schema::resolves_to_non_string(value) {
        return double_quoted(value, context);
    }

    value.to_owned()
}

/// yaml's `doubleQuotedString`: the JSON string, with YAML's short escapes, and with `\n` written
/// as a folded line break in a long value that is not a key.
fn double_quoted(value: &str, context: &Context) -> String {
    let json = json::stringify(&JsonValue::String(value.to_owned()));
    let bytes = json.as_bytes();
    let at = |i: usize| bytes.get(i).copied();
    let indent = if context.indent.is_empty() && DOCUMENT_MARKER.is_match(value) {
        INDENT_STEP
    } else {
        context.indent.as_str()
    };
    let mut out = String::new();
    let mut start = 0;
    let mut i = 0;

    while i < bytes.len() {
        let mut ch = bytes[i];

        if ch == b' ' && at(i + 1) == Some(b'\\') && at(i + 2) == Some(b'n') {
            // A space before a newline is escaped so folding does not eat it.
            out.push_str(&json[start..i]);
            out.push_str("\\ ");
            i += 1;
            start = i;
            ch = b'\\';
        }

        if ch == b'\\' {
            match at(i + 1) {
                Some(b'u') => {
                    out.push_str(&json[start..i]);

                    let code = &json[i + 2..i + 6];

                    match code {
                        "0000" => out.push_str("\\0"),
                        "0007" => out.push_str("\\a"),
                        "000b" => out.push_str("\\v"),
                        "001b" => out.push_str("\\e"),
                        "0085" => out.push_str("\\N"),
                        "00a0" => out.push_str("\\_"),
                        "2028" => out.push_str("\\L"),
                        "2029" => out.push_str("\\P"),
                        _ if code.starts_with("00") => {
                            out.push_str("\\x");
                            out.push_str(&code[2..]);
                        }
                        _ => out.push_str(&json[i..i + 6]),
                    }

                    i += 5;
                    start = i + 1;
                }
                Some(b'n') => {
                    if context.implicit_key
                        || at(i + 2) == Some(b'"')
                        || json.len() < DOUBLE_QUOTED_MIN_MULTI_LINE_LENGTH
                    {
                        i += 1;
                    } else {
                        // Folding eats the first newline.
                        out.push_str(&json[start..i]);
                        out.push_str("\n\n");

                        while at(i + 2) == Some(b'\\')
                            && at(i + 3) == Some(b'n')
                            && at(i + 4) != Some(b'"')
                        {
                            out.push('\n');
                            i += 2;
                        }

                        out.push_str(indent);

                        // A space after a newline is escaped so folding does not eat it.
                        if at(i + 2) == Some(b' ') {
                            out.push('\\');
                        }

                        i += 1;
                        start = i + 1;
                    }
                }
                _ => i += 1,
            }
        }

        i += 1;
    }

    if start == 0 {
        return json;
    }

    out.push_str(&json[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn folds_a_long_multi_line_value_inside_double_quotes() {
        let text =
            emit_yaml(&json!({ "description": "the first line of it\nthe second line of it" }));

        assert_eq!(
            text,
            "description: \"the first line of it\n\n  the second line of it\"\n"
        );
    }

    #[test]
    fn escapes_a_space_before_a_newline() {
        assert_eq!(emit_yaml(&json!(["a \nb"])), "- \"a\\ \\nb\"\n");
    }

    #[test]
    fn quotes_a_document_marker_key_only_at_the_top() {
        assert_eq!(emit_yaml(&json!({ "---": "v" })), "\"---\": v\n");
        assert_eq!(emit_yaml(&json!({ "a": { "---": "v" } })), "a:\n  ---: v\n");
        assert_eq!(emit_yaml(&json!("---")), "\"---\"\n");
    }
}
