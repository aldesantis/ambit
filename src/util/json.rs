//! JSON with JavaScript's semantics: `JSON.stringify` bytes and `JSON.parse` key order.
//!
//! Hook digests in `.ambit/state.json` and `ambit.lock` are hashes of `JSON.stringify` output, so
//! the serializer here is written by hand rather than through serde's: numbers print as JavaScript
//! prints them (`1`, not `1.0`; `1e+21`), and integer-like keys lead every object in ascending
//! numeric order, which is how a JavaScript object enumerates them whatever order they were
//! inserted in. String escaping is `serde_json`'s, which matches `JSON.stringify` for every string
//! Rust can hold.
//!
//! Integers above 2^53 keep full precision here where JavaScript would round them. That divergence
//! is accepted.

use std::cmp::Ordering;

pub type JsonValue = serde_json::Value;
pub type JsonObject = serde_json::Map<String, JsonValue>;

/// `JSON.stringify(v)`.
pub fn stringify(v: &JsonValue) -> String {
    let mut out = String::new();
    write_value(&mut out, v, None, 0);
    out
}

/// `JSON.stringify(v, null, 2)`, with no trailing newline.
pub fn stringify_pretty(v: &JsonValue) -> String {
    let mut out = String::new();
    write_value(&mut out, v, Some(2), 0);
    out
}

/// `JSON.parse(text)`.
///
/// A duplicate key keeps its first position and its last value. Integer-like keys move to the front
/// of each object in ascending numeric order, as a JavaScript object enumerates them.
///
/// # Errors
///
/// The `serde_json` error for text that is not JSON.
pub fn parse(text: &str) -> std::result::Result<JsonValue, serde_json::Error> {
    let mut value: JsonValue = serde_json::from_str(text)?;
    reorder_keys(&mut value);
    Ok(value)
}

/// Structural equality, ignoring key order, with numbers compared as JavaScript compares them.
///
/// What "unchanged" means for a format ambit can parse losslessly: a person may have reformatted
/// the file or reordered a server's keys, and neither counts as drift.
pub fn structurally_equal(a: &JsonValue, b: &JsonValue) -> bool {
    match (a, b) {
        (JsonValue::Number(x), JsonValue::Number(y)) => x.as_f64() == y.as_f64(),
        (JsonValue::Array(x), JsonValue::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| structurally_equal(p, q))
        }
        (JsonValue::Object(x), JsonValue::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|(key, value)| {
                    y.get(key)
                        .is_some_and(|other| structurally_equal(value, other))
                })
        }
        _ => a == b,
    }
}

/// Whether `key` is a canonical array index, which a JavaScript object enumerates first.
pub fn is_array_index(key: &str) -> bool {
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }

    if key.len() > 1 && key.starts_with('0') {
        return false;
    }

    key.parse::<u64>().is_ok_and(|n| n < u64::from(u32::MAX))
}

/// The keys of `object` in the order JavaScript enumerates them.
pub fn js_key_order(object: &JsonObject) -> Vec<&String> {
    let mut indices: Vec<&String> = object.keys().filter(|k| is_array_index(k)).collect();
    indices.sort_by(|a, b| compare_indices(a, b));
    indices.extend(object.keys().filter(|k| !is_array_index(k)));
    indices
}

/// Formats a number as JavaScript's `String(n)` does.
pub fn format_number(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }

    if let Some(u) = n.as_u64() {
        return u.to_string();
    }

    format_f64(n.as_f64().unwrap_or(0.0))
}

/// Formats a float as JavaScript's `String(n)` does. Non-finite values print as `null`, which is
/// what `JSON.stringify` writes for them.
pub fn format_f64(f: f64) -> String {
    if !f.is_finite() {
        return "null".to_owned();
    }

    if f == 0.0 {
        return "0".to_owned();
    }

    ryu_js::Buffer::new().format_finite(f).to_owned()
}

fn compare_indices(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn reorder_keys(value: &mut JsonValue) {
    match value {
        JsonValue::Array(items) => items.iter_mut().for_each(reorder_keys),
        JsonValue::Object(object) => {
            if object.keys().any(|k| is_array_index(k)) {
                let mut taken = std::mem::take(object);
                let order: Vec<String> = js_key_order(&taken).into_iter().cloned().collect();

                for key in order {
                    if let Some(item) = taken.remove(&key) {
                        object.insert(key, item);
                    }
                }
            }

            object.values_mut().for_each(reorder_keys);
        }
        _ => {}
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push_str(&serde_json::to_string(s).expect("serializing a string cannot fail"));
}

fn newline(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(width) = indent {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', width * depth));
    }
}

fn write_value(out: &mut String, value: &JsonValue, indent: Option<usize>, depth: usize) {
    match value {
        JsonValue::Null => out.push_str("null"),
        JsonValue::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        JsonValue::Number(n) => out.push_str(&format_number(n)),
        JsonValue::String(s) => write_string(out, s),
        JsonValue::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }

            out.push('[');

            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }

                newline(out, indent, depth + 1);
                write_value(out, item, indent, depth + 1);
            }

            newline(out, indent, depth);
            out.push(']');
        }
        JsonValue::Object(object) => {
            if object.is_empty() {
                out.push_str("{}");
                return;
            }

            out.push('{');

            for (index, key) in js_key_order(object).into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }

                newline(out, indent, depth + 1);
                write_string(out, key);
                out.push(':');

                if indent.is_some() {
                    out.push(' ');
                }

                write_value(out, &object[key.as_str()], indent, depth + 1);
            }

            newline(out, indent, depth);
            out.push('}');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_the_recorded_json_stringify_corpus() {
        let corpus = include_str!("../../tests/fixtures/json_stringify.json");
        let cases: JsonValue = serde_json::from_str(corpus).expect("fixture is JSON");

        for case in cases.as_array().expect("fixture is an array") {
            let source = case["source"].as_str().expect("source");
            let value = parse(source).expect("source parses");

            assert_eq!(
                stringify(&value),
                case["compact"].as_str().unwrap(),
                "compact: {source}"
            );
            assert_eq!(
                stringify_pretty(&value),
                case["pretty"].as_str().unwrap(),
                "pretty: {source}"
            );
            assert_eq!(
                &crate::util::hash::sha256_hex(stringify(&value).as_bytes())[..12],
                case["digest"].as_str().unwrap(),
                "digest: {source}"
            );
        }
    }

    #[test]
    fn orders_integer_keys_of_constructed_objects_like_javascript() {
        assert_eq!(
            stringify(&json!({"b": 1, "10": 2, "2": 3})),
            r#"{"2":3,"10":2,"b":1}"#
        );
    }

    #[test]
    fn parse_moves_integer_keys_first() {
        let value = parse(r#"{"b":1,"2":0}"#).unwrap();
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();

        assert_eq!(keys, ["2", "b"]);
    }

    #[test]
    fn compares_structurally_ignoring_key_order() {
        assert!(structurally_equal(
            &json!({"a": 1, "b": [1.0]}),
            &json!({"b": [1], "a": 1.0})
        ));
        assert!(!structurally_equal(
            &json!({"a": 1}),
            &json!({"a": 1, "b": 2})
        ));
        assert!(!structurally_equal(&json!([1, 2]), &json!([2, 1])));
        assert!(!structurally_equal(&json!("1"), &json!(1)));
    }

    #[test]
    fn recognizes_array_indices() {
        assert!(is_array_index("0"));
        assert!(is_array_index("4294967294"));
        assert!(!is_array_index("4294967295"));
        assert!(!is_array_index("01"));
        assert!(!is_array_index("-1"));
        assert!(!is_array_index(""));
    }
}
